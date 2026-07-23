# rust-fmi_SE — FMI 3.0 Scheduled Execution para `rust-fmi`

Este repositorio contiene una versión de [`rust-fmi`](https://github.com/samgiles/rust-fmi)
con la interfaz **FMI 3.0 Scheduled Execution (SE)** implementada en el **exportador**
(`fmi-export`), más varios **modelos de ejemplo** y una **guía paso a paso**.

Con esto, una FMU de Scheduled Execution (p. ej. un modelo aperiódico tipo DEVS con un
reloj *countdown*) se escribe igual que cualquier otra FMU `rust-fmi`: un `struct` con
`#[derive(FmuModel)]`, un `impl UserModel` con la física, y `export_fmu!`. El
`modelDescription.xml` y todo el ABI C se generan automáticamente.

> **Contexto:** antes, el motor de SE de `fmi-export` estaba sin implementar (instanciar
> una FMU SE era un `todo!()`, activar una partición devolvía `Error`, y las funciones de
> intervalo de reloj eran `todo!()` = pánico a través de FFI). Este repo lo implementa de
> punta a punta. El informe completo está en
> [`SE_IMPLEMENTACION.md`](SE_IMPLEMENTACION.md).

---

## ¿Qué añade respecto a `rust-fmi`?

En `rust-fmi/fmi-export` (el **exportador**):

- **Instanciar/liberar** una FMU SE (`fmi3InstantiateScheduledExecution`, `fmi3FreeInstance`).
- **`fmi3ActivateModelPartition`** — el "do_step" de SE.
- **Relojes countdown**: `fmi3GetIntervalDecimal` / `fmi3GetIntervalFraction` con la
  gestión correcta del *qualifier* por reloj (`Changed`/`Unchanged`/`NotYetKnown`).
- `fmi3GetShiftDecimal/Fraction`, `fmi3EvaluateDiscreteStates`; y las funciones `set` de
  reloj devuelven un `fmi3Error` seguro en vez de provocar UB.
- Dos *hooks* nuevos en el trait `UserModel`: **`activate_partition`** y
  **`next_interval`** (ambos con implementación por defecto, así que no rompen ningún
  modelo existente).
- Corrección en `variable_builder`: el atributo `clocks` ahora se propaga a variables
  float/int/bool (antes solo a binarias), necesario para declarar salidas *relojadas*.

Todo verificado con un test de integración por el ABI C
(`rust-fmi/fmi-export/tests/test_scheduled_execution.rs`), incluido un caso multi-reloj.

---

## Estructura del repositorio

```
rust-fmi_SE/
├── README.md                 ← este archivo
├── SE_IMPLEMENTACION.md       ← informe completo de la implementación (qué/dónde/cómo/por qué)
├── SE_PLAN.md                 ← análisis y plan previo
├── rust-fmi/                  ← la librería (fork con SE implementado)
│   ├── fmi/                   ← crate núcleo FMI
│   ├── fmi-export/            ← EXPORTADOR (aquí vive la implementación de SE)
│   ├── fmi-export-derive/     ← macro #[derive(FmuModel)]
│   ├── cargo-fmi/             ← herramienta CLI para empaquetar/inspeccionar FMUs
│   └── …
├── src/lib.rs + Cargo.toml    ← ejemplo: motor térmico (Co-Simulation)
├── semaforo_se/               ← ejemplo: semáforo aperiódico (SE), 60/30 s
├── semaforoV2_se/             ← ejemplo: semáforo V2 (SE), 30/15 s
│   ├── src/lib.rs             ← el modelo
│   ├── examples/simular.rs    ← mini-planificador SE para simular el comportamiento
│   └── GUIA_PASO_A_PASO.md    ← tutorial: cómo crear una FMU de SE desde cero
├── boton_pwm_cs/              ← ejemplo: generador de botón, pulso/PWM/escalón (CS)
└── acoplado_sim/              ← CO-SIMULACIÓN ACOPLADA: generador (CS) + semáforo (SE)
```

> Cada carpeta de ejemplo es un crate independiente que depende de `rust-fmi/` por rutas
> relativas (`../rust-fmi/...`). No hay un *workspace* raíz porque `rust-fmi/` ya es uno.

---

## Requisitos

- **Rust** (edición 2021; toolchain estable reciente). Instálalo con
  [rustup](https://rustup.rs/).
- En Windows, el toolchain MSVC (por defecto con rustup) funciona bien.

---

## Empezar rápido

### 1. Compilar y probar la librería (con los tests de SE)

```bash
cd rust-fmi
cargo test -p fmi-export --features fmi3
```
Debe pasar todo, incluidos los tests de `test_scheduled_execution` (ciclo SE completo por
el ABI C, intervalos fraccionarios, y multi-reloj).

### 2. Instalar la herramienta `cargo fmi` (una vez)

```bash
cargo install --path rust-fmi/cargo-fmi
```
Deja disponible el subcomando `cargo fmi` para empaquetar e inspeccionar FMUs.

### 3. Generar un FMU de ejemplo

```bash
cd semaforoV2_se
cargo test                              # prueba la lógica del modelo
cargo fmi bundle -p semaforoV2_se       # genera target/fmu/semaforoV2_se.fmu (+ modelDescription.xml)
cargo fmi inspect target/fmu/semaforoV2_se.fmu --format model-description
```

### 4. Simular el comportamiento de una FMU SE

`fmi-sim` (el simulador incluido) soporta Co-Simulation y Model Exchange, **pero no
Scheduled Execution** (su rama SE es `unimplemented!()`). Para ver el comportamiento hay un
mini-planificador de ejemplo:

```bash
cd semaforoV2_se
cargo run --example simular
```
Imprime la traza temporal del semáforo (rojo/verde y el efecto del botón), llamando al FMU
por su ABI C real — el mismo rol que hará un orquestador externo.

---

## Los ejemplos

| Ejemplo | Interfaz | Qué demuestra |
|---|---|---|
| `src/` (motor térmico) | Co-Simulation | Modelo continuo (calentamiento con la carga), generado con `#[derive(FmuModel)]`. |
| `semaforo_se/` | Scheduled Execution | Semáforo aperiódico DEVS (rojo 60 s / verde 30 s, botón → 15 s) con reloj countdown. |
| `semaforoV2_se/` | Scheduled Execution | Igual pero 30/15/20 s; incluye tutorial y simulador de ejemplo. |
| `boton_pwm_cs/` | Co-Simulation | Generador de señal de botón: pulso único, PWM o escalón (parametrizable). |
| `acoplado_sim/` | — (orquestador) | Co-simula las dos FMUs juntas; referencia de cómo acoplar CS ↔ SE. |

Para aprender a construir una FMU de SE desde cero, sigue
[`semaforoV2_se/GUIA_PASO_A_PASO.md`](semaforoV2_se/GUIA_PASO_A_PASO.md).

---

## Co-simulación acoplada CS ↔ SE (`acoplado_sim/`)

Conecta el generador de botón (CS) con el semáforo (SE):

```text
boton_pwm_cs.salida (VR 5)  ──►  semaforoV2_se.boton (VR 1)
```

```bash
cd boton_pwm_cs  && cargo build && cd ..     # genera boton_pwm_cs.dll
cd semaforoV2_se && cargo build && cd ..     # genera semaforoV2_se.dll
cd acoplado_sim  && cargo run
```

Traza real (semáforo 30/15 s, botón → rojo de 20 s en total; generador con pulso de 2 s
cada 40 s desde t=8):

```text
  t (s) | fase    |  σ (s) | botón | evento
  ------+---------+--------+-------+------------------------------
    0.0 | 🔴 ROJO  |   30.0 |    0  | inicio
    8.0 | 🔴 ROJO  |   12.0 |    1  | δext: 👆 BOTÓN pulsado     ← σ = 20-8
   20.0 | 🟢 VERDE |   15.0 |    0  | δint: fin de fase          ← rojo duró 20 s
   35.0 | 🔴 ROJO  |   30.0 |    0  | δint: fin de fase
   48.0 | 🔴 ROJO  |    7.0 |    1  | δext: 👆 BOTÓN pulsado     ← σ = 20-13
   55.0 | 🟢 VERDE |   15.0 |    0  | δint: fin de fase          ← rojo duró 20 s
  …
  128.0 | 🔴 ROJO  |    0.0 |    1  | δext: 👆 BOTÓN pulsado     ← rojo ya llevaba 23 s
  128.0 | 🟢 VERDE |   15.0 |    1  | δint: fin de fase          ← cambio INMEDIATO
```

### ⚠️ Las dos reglas críticas al acoplar CS con SE

Si vas a escribir tu propio orquestador, esto es lo que hay que respetar:

1. **Una FMU SE solo reacciona cuando se activa su partición.** No basta con escribirle la
   entrada con `fmi3SetFloat64`: hay que llamar a `fmi3ActivateModelPartition` **en el
   instante en que la entrada cambia** (δext). Si solo se activa al vencer el countdown, la
   pulsación del botón **no tiene ningún efecto**.
2. **Hay que releer `fmi3GetIntervalDecimal` después de CADA activación** (interna o
   externa) y reprogramar el próximo evento en `t + σ`.

Es decir, el planificador fusiona **dos fuentes de eventos**: el countdown del semáforo
(δint) y los flancos de la entrada procedente del CS (δext).

### Nota: dos FMUs no se pueden enlazar en un mismo binario

Cada FMU exporta los mismos símbolos C (`fmi3GetFloat64`, `fmi3Terminate`, …), así que
enlazar dos crates de FMU en un ejecutable da error de *símbolo duplicado*. Por eso
`acoplado_sim` las **carga como bibliotecas dinámicas en tiempo de ejecución**
(`libloading`), que es lo que hace cualquier orquestador real.

---

## Cómo se escribe una FMU de Scheduled Execution (resumen)

```rust
#[derive(FmuModel, Debug)]
#[model(scheduled_execution = true, model_exchange = false, co_simulation = false, user_model = false)]
struct MiModelo {
    #[variable(causality = Input, interval_variability = Countdown)]
    reloj: Clock,                              // reloj countdown que dispara la partición
    #[variable(causality = Output, variability = Discrete, start = 0.0, clocks = [reloj])]
    salida: f64,                               // salida relojada
    sigma: f64,                                // estado interno (sin atributo → invisible)
}

impl UserModel for MiModelo {
    type LoggingCategory = DefaultLoggingCategory;
    fn calculate_values(&mut self, _c) -> Result<Fmi3Res, Fmi3Error> { /* salidas ← estado */ Ok(Fmi3Res::OK) }
    fn activate_partition(&mut self, _c, _clk, t) -> Result<Fmi3Res, Fmi3Error> { /* δ + λ */ Ok(Fmi3Res::OK) }
    fn next_interval(&self, _clk) -> Option<f64> { Some(self.sigma) }   // ta() de DEVS
}
fmi_export::export_fmu!(MiModelo);
```

Mapeo DEVS ↔ FMI 3.0 SE:

| DEVS | FMI 3.0 SE | Hook `UserModel` |
|---|---|---|
| `ta()` = σ | `fmi3GetIntervalDecimal` | `next_interval` |
| `δint + λ` | `fmi3ActivateModelPartition` (σ agotada) | `activate_partition` |
| `δext` | `fmi3ActivateModelPartition` (σ>0, por una entrada) | `activate_partition` |
| salidas | `fmi3GetFloat64` | `calculate_values` |

---

## Documentación

- **[`SE_IMPLEMENTACION.md`](SE_IMPLEMENTACION.md)** — informe completo: qué se cambió,
  dónde, cómo y por qué; decisiones de diseño; validación.
- **[`semaforoV2_se/GUIA_PASO_A_PASO.md`](semaforoV2_se/GUIA_PASO_A_PASO.md)** — tutorial de
  7 pasos para crear una FMU de SE desde cero.
- **[`SE_PLAN.md`](SE_PLAN.md)** — el análisis previo (mapa del código y plan).

---

## Notas

- Los *Value References* que asigna `#[derive(FmuModel)]` son **secuenciales** (el `time`
  es 0, luego 1, 2, 3…). Un importador debe leerlos del `modelDescription.xml`, no fijarlos
  a mano.
- El aviso de compilación `unexpected cfg 'coverage_nightly'` es inofensivo (viene de la
  macro `export_fmu!`).

## Licencia y créditos

`rust-fmi` se distribuye bajo licencia Apache-2.0 / MIT (ver `rust-fmi/LICENSE-*`). La
implementación de Scheduled Execution y los ejemplos de este repositorio se publican bajo
las mismas condiciones.
