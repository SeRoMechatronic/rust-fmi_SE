# Guía paso a paso: crear una FMU de Scheduled Execution con `rust-fmi`

Tutorial práctico usando el **semáforo V2** como ejemplo. Al final tendrás una FMU
FMI 3.0 de *Scheduled Execution* con un reloj *countdown*, generada íntegramente desde
un modelo escrito en Rust.

**Comportamiento del semáforo V2 (un átomo DEVS):**
- ROJO **30 s** → VERDE **15 s** → ROJO 30 s → …
- Si se pulsa el BOTÓN durante el rojo, el rojo pasa a valer **20 s en total**.

**El flujo general (lo más importante):**

```
1. Escribes el MODELO en Rust  ── struct + impl UserModel + export_fmu!
2. cargo test                  ── pruebas la lógica (sin FFI)
3. cargo fmi bundle            ── compila la .dll Y genera el modelDescription.xml → .fmu
4. cargo fmi inspect           ── verificas el XML generado
```

> Clave: **el `modelDescription.xml` se genera solo** a partir del struct de Rust. No se
> escribe XML a mano.

Una FMU de SE con `fmi-export` tiene **3 piezas** en `src/lib.rs`:
1. **El `struct`** con `#[derive(FmuModel)]` → la *interfaz* (qué ve el simulador).
2. **`impl UserModel`** → la *física* (la lógica DEVS).
3. **`export_fmu!(...)`** → genera todo el ABI C.

---

## Paso 0 — Crear el proyecto

Una carpeta con un `Cargo.toml` que produce una biblioteca dinámica (`cdylib`) y depende
de `fmi` y `fmi-export`:

```toml
[package]
name = "semaforoV2_se"
version = "0.1.0"
edition = "2021"
description = "Semáforo V2 — FMI 3.0 Scheduled Execution (reloj countdown)"

[lib]
crate-type = ["cdylib"]          # ← una FMU es una biblioteca dinámica

[dependencies]
fmi        = { path = "../rust-fmi/fmi",        features = ["fmi3"], default-features = false }
fmi-export = { path = "../rust-fmi/fmi-export", features = ["fmi3"] }

[package.metadata.fmu]
# Experimento por defecto que irá al XML (opcional).
default_experiment = { start_time = "0", stop_time = "180", step_size = "1" }
```

`crate-type = ["cdylib"]` es lo que convierte tu crate en una `.dll`/`.so` cargable por el
importador. Las dos dependencias son la librería (`fmi`) y el exportador (`fmi-export`).

---

## Paso 1 — El `struct`: la interfaz del modelo

Aquí decides **qué ve el mundo exterior** y **qué estado interno** guardas.

> **Regla de oro:** un campo **con** `#[variable(...)]` aparece en el XML (interfaz FMI);
> un campo **sin** atributo es estado interno y el `derive` lo **ignora**.

```rust
use fmi::fmi3::{binding::fmi3ValueReference, Fmi3Error, Fmi3Res};
use fmi_export::{
    fmi3::{Clock, Context, DefaultLoggingCategory, UserModel},
    FmuModel,
};

// Parámetros del modelo
const ROJO_S: f64 = 30.0;
const VERDE_S: f64 = 15.0;
const ROJO_CON_BOTON_S: f64 = 20.0;
const EPS: f64 = 1e-9;

// La fase es un estado discreto: un enum sencillo.
#[derive(PartialEq, Clone, Copy, Debug, Default)]
enum Fase { #[default] Rojo, Verde }

#[derive(FmuModel, Debug)]
#[model(model_exchange = false, co_simulation = false,
        scheduled_execution = true, user_model = false)]
struct SemaforoV2 {
    #[variable(causality = Input, variability = Discrete, start = 0.0)]
    boton: f64,

    #[variable(causality = Input, interval_variability = Countdown)]
    reloj: Clock,

    #[variable(causality = Output, variability = Discrete, start = 1.0, clocks = [reloj])]
    rojo: f64,
    #[variable(causality = Output, variability = Discrete, start = 0.0, clocks = [reloj])]
    verde: f64,
    #[variable(causality = Output, variability = Discrete, start = 30.0, clocks = [reloj])]
    t_restante: f64,

    // Estado interno DEVS (sin atributos → invisible para el derive)
    fase: Fase,
    sigma: f64,
    elapsed: f64,
    t_last: f64,
    boton_atendido: bool,
}
```

**Qué significa cada cosa:**

- `#[model(...)]`:
  - `scheduled_execution = true` → es una FMU de SE.
  - `user_model = false` → *tú* escribes `impl UserModel` (no lo autogenera el `derive`).
- `#[variable(...)]`:
  - `causality = Input` → el maestro la escribe (`boton`).
  - `reloj: Clock` + `interval_variability = Countdown` → el **reloj countdown** que dispara
    la partición; su intervalo lo decide la FMU.
  - `causality = Output` + `clocks = [reloj]` → **salidas relojadas**: solo cambian en los
    *ticks* de `reloj` (así se declara en el XML).
  - `start = ...` → valor inicial (rojo=1, verde=0, t_restante=30 → arranca en rojo, 30 s).
- **Value References:** el `derive` los numera por orden de campo tras `time`(=0):
  `boton`=1, `reloj`=2, `rojo`=3, `verde`=4, `t_restante`=5.

### Mapeo DEVS ↔ FMI 3.0 SE (la idea central)

| DEVS | FMI 3.0 SE | Hook `UserModel` |
|---|---|---|
| `ta()` = σ restante | `fmi3GetIntervalDecimal(reloj)` | `next_interval` |
| `δint + λ` (fin de fase) | `fmi3ActivateModelPartition` con σ agotada | `activate_partition` |
| `δext` (cambió una entrada) | `fmi3ActivateModelPartition` con σ>0 (botón) | `activate_partition` |
| salidas = f(estado) | `fmi3GetFloat64` | `calculate_values` |

> En este punto el archivo **aún no compila**: faltan las piezas 2 y 3.

---

## Paso 2 — `impl Default`: el estado inicial

`#[derive(FmuModel)]` **exige** que el struct implemente `Default` (al instanciar, la
librería hace `M::default()`). Lo escribimos a mano para arrancar el estado DEVS en valores
concretos (rojo, σ=30) en vez de ceros.

```rust
impl Default for SemaforoV2 {
    fn default() -> Self {
        Self {
            boton: 0.0, reloj: Clock::default(),
            rojo: 1.0, verde: 0.0, t_restante: ROJO_S,
            fase: Fase::Rojo, sigma: ROJO_S,
            elapsed: 0.0, t_last: 0.0, boton_atendido: false,
        }
    }
}
```

**Por qué así:**
- Al instanciar, la librería hace `M::default()` y *después* `set_start_values()` (generado
  por el `derive`, fija los campos `#[variable]` a su `start=`). Los valores conviven bien.
- Se inicializa en `Default` y **no** en `configurate` porque al salir de inicialización
  `fmi-export` llama a `calculate_values` **antes** que a `configurate`; el estado debe estar
  listo ya al construir, o las salidas se calcularían con `sigma=0`.

---

## Paso 3 — La lógica DEVS (`fn activar`)

El corazón del semáforo. El planificador llama a esta función en cada *tick* de la
partición; ella decide si es un cambio **interno** (se agotó la fase) o **externo** (llegó
el botón).

```rust
impl SemaforoV2 {
    fn activar(&mut self, t: f64) {
        let e = (t - self.t_last).max(0.0); // tiempo desde la última activación
        self.t_last = t;
        self.sigma = (self.sigma - e).max(0.0); // consumimos σ
        self.elapsed += e;                       // avanzamos dentro de la fase

        if self.sigma <= EPS {
            // δint + λ: se agotó la fase → cambiar
            match self.fase {
                Fase::Rojo  => { self.fase = Fase::Verde; self.sigma = VERDE_S; } // 15 s
                Fase::Verde => { self.fase = Fase::Rojo;  self.sigma = ROJO_S;  } // 30 s
            }
            self.elapsed = 0.0;
            self.boton_atendido = false;
        } else {
            // δext: activación por el botón
            if self.fase == Fase::Rojo && self.boton >= 0.5 && !self.boton_atendido {
                self.boton_atendido = true;
                self.sigma = (ROJO_CON_BOTON_S - self.elapsed).max(0.0); // rojo = 20 s total
            }
        }
    }
}
```

**La idea:**
1. `e` = tiempo transcurrido desde la última activación → consume `σ` y avanza `elapsed`.
2. **`σ ≤ 0` → transición interna:** la fase acabó. Rojo→Verde (σ=15) o Verde→Rojo (σ=30);
   se resetea `elapsed` y `boton_atendido`.
3. **`σ > 0` → transición externa (botón):** solo en rojo, una vez. `σ = (20 − elapsed)` →
   el rojo dura **20 s en total**.

**Ejemplos (parámetros de V2):**

| Botón | `elapsed` | σ tras el botón | Efecto |
|---|---|---|---|
| a los 8 s de rojo | 8 | 20−8 = **12** | cambia a verde a los 20 s |
| a los 25 s de rojo | 25 | max(20−25,0) = **0** | cambio a verde **inmediato** (σ=0 → re-activación ya) |
| durante el verde | — | (no aplica) | se **ignora** |

> Truco elegante: el "cambio inmediato" no es un caso especial; dejamos `σ=0` y la propia
> mecánica DEVS lo resuelve en la siguiente re-activación (mismo instante) por la rama δint.

## Paso 4 — `impl UserModel`: los 3 hooks

Aquí conectas tu lógica con el motor SE. El motor de `fmi-export` llama estos métodos por ti.

```rust
impl UserModel for SemaforoV2 {
    type LoggingCategory = DefaultLoggingCategory;

    // Responde a fmi3GetFloat64: salidas ← estado
    fn calculate_values(&mut self, _c: &dyn Context<Self>) -> Result<Fmi3Res, Fmi3Error> {
        self.rojo = if self.fase == Fase::Rojo { 1.0 } else { 0.0 };
        self.verde = if self.fase == Fase::Verde { 1.0 } else { 0.0 };
        self.t_restante = self.sigma;
        Ok(Fmi3Res::OK)
    }

    // Responde a fmi3ActivateModelPartition: un tick de la partición
    fn activate_partition(&mut self, _c: &mut dyn Context<Self>,
                          _clock: fmi3ValueReference, time: f64) -> Result<Fmi3Res, Fmi3Error> {
        self.activar(time);
        Ok(Fmi3Res::OK)
    }

    // Responde a fmi3GetIntervalDecimal: ta() = σ
    fn next_interval(&self, _clock: fmi3ValueReference) -> Option<f64> {
        Some(self.sigma)
    }
}
```

**Qué hace cada hook y con qué llamada C se conecta:**

| Hook | Llamada C | Qué hace |
|---|---|---|
| `calculate_values` | `fmi3GetFloat64` | Calcula las salidas desde el estado (perezoso: solo si está "sucio"). |
| `activate_partition` | `fmi3ActivateModelPartition` | El "do_step" de SE: llama a `activar(time)`. |
| `next_interval` | `fmi3GetIntervalDecimal` | Devuelve `Some(σ)` — el `ta()` de DEVS. |

**Lo que el motor ya hace por ti** (no lo escribes): verificar el estado, `set_time`, marcar
salidas sucias, sacar el reloj del set de "observados", y decidir el *qualifier* del
intervalo (`Changed`/`Unchanged`/`NotYetKnown`).

**El bucle completo, conectado:**
```
fmi3ActivateModelPartition(reloj, t) → activate_partition → activar(t)   [cambia fase/σ]
fmi3GetIntervalDecimal(reloj)        → next_interval → Some(σ)           [ta()]
fmi3GetFloat64(rojo/verde/...)       → calculate_values                 [salidas]
```

## Paso 5 — `export_fmu!` + tests

Una sola línea genera **todo el ABI C** de la FMU, y además el símbolo `model_metadata`
que `cargo fmi` usará para construir el `modelDescription.xml`:

```rust
fmi_export::export_fmu!(SemaforoV2);
```

Esto genera `fmi3InstantiateScheduledExecution`, `fmi3ActivateModelPartition`,
`fmi3GetFloat64`/`SetFloat64`, `fmi3GetIntervalDecimal`, `fmi3FreeInstance`, etc. (lo que en
la versión a mano eran ~15 funciones `unsafe extern "C"`).

Los tests prueban la **lógica DEVS pura** (sin FFI):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciclo_normal_30_15() {
        let mut s = SemaforoV2::default();
        s.activar(30.0); assert_eq!(s.fase, Fase::Verde); // fin del rojo → verde 15 s
        s.activar(45.0); assert_eq!(s.fase, Fase::Rojo);  // fin del verde → rojo 30 s
    }

    #[test]
    fn boton_temprano_acorta_a_20() {
        let mut s = SemaforoV2::default();
        s.boton = 1.0;
        s.activar(8.0);                              // botón a los 8 s (elapsed 8 < 20)
        assert!((s.sigma - 12.0).abs() < 1e-9);      // quedan 20-8 = 12 s de rojo
        s.activar(20.0); assert_eq!(s.fase, Fase::Verde);
    }
    // (+ boton_tardio_cambia_inmediato, boton_en_verde_se_ignora)
}
```

Ejecuta:

```bash
cd semaforoV2_se
cargo test
```

> A partir de aquí el modelo **ya compila** (están las 3 piezas). Las *warnings*
> `cfg(coverage_nightly)` son inofensivas.

### ¿Para qué sirven los tests? (¿son obligatorios?)

**No, son opcionales** — no forman parte de la FMU. El bloque `#[cfg(test)]` significa
"compila esto solo con `cargo test`"; al empaquetar (`cargo fmi bundle`) ese código ni
existe. Puedes borrarlo y la FMU funciona igual.

Sirven para **verificar tu lógica DEVS antes** de meterla en la maquinaria pesada (FFI +
orquestador):

| | `export_fmu!` | tests `#[cfg(test)]` |
|---|---|---|
| Qué es | el producto (ABI C de la FMU) | control de calidad de la lógica |
| ¿va en el `.fmu`? | sí | **no, nunca** |
| ¿cuándo compila? | siempre | solo con `cargo test` |

- **Sin tests:** para saber si el semáforo funciona tendrías que empaquetar el `.fmu`,
  cargarlo en el orquestador y *mirar* si el rojo dura 30 s; si falla, no sabes si es la
  lógica, el FFI o el orquestador.
- **Con tests:** llamas a `activar()` directamente sobre el struct (sin FFI) y compruebas
  los campos en milisegundos. Aíslan el "cerebro" DEVS y atrapan regresiones si cambias los
  tiempos.

Regla práctica: `export_fmu!` es **obligatorio**; los tests son **opcionales pero muy
recomendables**.

## Paso 6 — `bundle` + `inspect`: generar y ver el FMU

Aquí NO se escribe código: se **genera** el `.fmu` desde el modelo Rust y se verifica.

### 6.1 (una vez) instalar la herramienta `cargo fmi`

```bash
cargo install --path ../rust-fmi/cargo-fmi
```
Deja disponible el subcomando `cargo fmi`. (Alternativa sin instalar: llamar
`../rust-fmi/target/debug/cargo-fmi.exe` directamente.)

### 6.2 empaquetar el FMU

```bash
cd semaforoV2_se
cargo fmi bundle -p semaforoV2_se
```
Esto compila la `.dll`, **lee el símbolo `model_metadata` y genera el
`modelDescription.xml`** automáticamente, y empaqueta todo en
`target/fmu/semaforoV2_se.fmu`. En el log verás `Extracted 6 model variables`.

### 6.3 inspeccionar el XML generado

```bash
cargo fmi inspect target/fmu/semaforoV2_se.fmu --format model-description
```

Salida (abreviada) — **todo esto salió del struct, no se escribió a mano**:

```xml
<ScheduledExecution modelIdentifier="semaforoV2_se"/>
<Clock name="reloj" valueReference="2" causality="input"
       variability="discrete" intervalVariability="countdown"/>
<Float64 name="rojo"       valueReference="3" causality="output" ... clocks="2" start="1"/>
<Float64 name="verde"      valueReference="4" causality="output" ... clocks="2" start="0"/>
<Float64 name="t_restante" valueReference="5" causality="output" ... clocks="2" start="30"/>
```

**Qué comprobar (checklist):**
- `<ScheduledExecution>` presente (y no ME/CS).
- El `<Clock>` con `intervalVariability="countdown"` y `causality="input"`.
- Las salidas con `clocks="N"` apuntando al VR del reloj.
- Los Value References: `time`=0, `boton`=1, `reloj`=2, `rojo`=3, `verde`=4, `t_restante`=5.

> **Detalle:** el aviso *"crate should have a snake case name"* es solo estilo de Rust; la
> FMU funciona igual. Si molesta, renombra el paquete a `semaforo_v2_se`.

---

## Paso 7 — Simular el comportamiento (sin SeRo_CoSim)

**Importante:** el simulador de rust-fmi, `fmi-sim`, ejecuta Co-Simulation y Model Exchange,
pero su rama de Scheduled Execution es `unimplemented!()` (`fmi-sim/src/sim/mod.rs`). Un FMU
de SE lo mueve un *planificador* (scheduler), que es el "lado importador" (aún no hecho para
SE en rust-fmi).

Para ver el comportamiento ya, hay un **mini-planificador de ejemplo** en
`examples/simular.rs`: instancia el FMU, lee el intervalo countdown (`ta()`), programa la
siguiente activación en `t+σ`, activa la partición, lee las salidas, y repite; además inyecta
una pulsación de botón. Llama al FMU **por su ABI C real** (lo mismo que hará SeRo_CoSim).

Para poder enlazar el crate desde `examples/`, el `Cargo.toml` añade `rlib`:
```toml
[lib]
crate-type = ["cdylib", "rlib"]
```
y el struct se hace público: `pub struct SemaforoV2 { … }`.

Ejecutar:
```bash
cargo run --example simular
```

Salida esperada (rojo 30 → verde 15, pero el botón a los 10 s acorta el rojo a 20 s total):
```
  t (s) | estado  |  σ (s) hasta el próximo cambio
  ------+---------+-------------------------------
    0.0 | 🔴 ROJO  |   30.0
        |  👆 botón pulsado → el rojo se acorta a 20 s en total
   10.0 | 🔴 ROJO  |   10.0
   20.0 | 🟢 VERDE |   15.0
   35.0 | 🔴 ROJO  |   30.0
   65.0 | 🟢 VERDE |   15.0
   80.0 | 🔴 ROJO  |   30.0
```

El núcleo del bucle (planificador countdown):
```rust
let sigma = get_interval(inst);          // ta(): fmi3GetIntervalDecimal
let t_evento = t + sigma;                // próximo evento interno
activar_particion(inst, t_evento);       // fmi3ActivateModelPartition (δint)
t = t_evento;
// (y una pulsación de botón = activación con σ>0 → δext)
```

## Resumen del flujo completo

```
struct + #[derive(FmuModel)]   ┐
impl Default                   ├─ escribes el MODELO en Rust (src/lib.rs)
fn activar (lógica DEVS)       │
impl UserModel (3 hooks)       │
export_fmu!                    ┘
        │
        ├─ cargo test                → prueba la lógica (opcional pero recomendable)
        ├─ cargo fmi bundle          → .fmu + modelDescription.xml (automático)
        └─ cargo fmi inspect         → verificar el XML
```

¡Y listo! Tienes una FMU FMI 3.0 de Scheduled Execution generada 100 % desde Rust.
