# Implementación de Scheduled Execution (SE) en `fmi-export` — informe completo

> Documento de resultado. Acompaña a [`SE_PLAN.md`](SE_PLAN.md) (el análisis previo).
> Aquí se explica **qué se cambió, dónde, cómo y por qué**, de forma que cualquier
> persona pueda seguir y reproducir el trabajo. Todo lo descrito está compilado y
> probado (ver §7 Validación).

---

## 0. TL;DR

El exportador de FMUs `rust-fmi/fmi-export` **declaraba** la interfaz FMI 3.0
*Scheduled Execution* (el flag, los símbolos C exportados, `intervalVariability="Countdown"`
en la macro) pero su **motor estaba vacío**: instanciar una FMU SE era un `todo!()`,
activar una partición devolvía `Error` y las funciones de intervalo de reloj eran
`todo!()` (pánico a través de FFI = comportamiento indefinido). Por eso el semáforo
(`semaforo_se`) hubo que escribirlo con el ABI FMI a mano.

Ahora **el motor SE está implementado de punta a punta**. Se puede escribir una FMU de
Scheduled Execution exactamente igual que una de Co-Simulation: `#[derive(FmuModel)]`
sobre el struct + `impl UserModel` con la física + `export_fmu!`. Como prueba, el
semáforo se reescribió con la macro (169 líneas legibles en vez del ABI a mano) y
genera un `modelDescription.xml` estándar con su reloj *countdown* y sus salidas
relojadas.

**Lo que se tocó (8 archivos):**

| # | Archivo | Cambio |
|---|---------|--------|
| 1 | `fmi-export/src/fmi3/traits/mod.rs` | 2 hooks nuevos en `UserModel`: `activate_partition`, `next_interval` (con defaults) |
| 2 | `fmi-export/src/fmi3/instance/mod.rs` | Campo `intervals_observed` (set por-reloj) en `ModelInstance` |
| 3 | `fmi-export/src/fmi3/instance/common.rs` | `reset()` reinicia el estado |
| 4 | `fmi-export/src/fmi3/instance/impl_se.rs` | **Núcleo SE**: `activate_model_partition` + helpers de intervalo/shift/discrete |
| 5 | `fmi-export/src/fmi3/traits/wrappers.rs` | `fmi3InstantiateScheduledExecution`, `fmi3FreeInstance` (SE) y los 9 wrappers de reloj/discrete |
| 6 | `fmi-export/src/fmi3/variable_builder.rs` | Propagar el atributo `clocks` en variables float/int/bool |
| 7 | `fmi-export/src/fmi3/export.rs` | `fmi3GetIntervalDecimal` pasa a ser `extern "C"` (arreglo de ABI, ver §4.6) |
| 8 | `semaforo_se/{Cargo.toml,src/lib.rs}` | Semáforo reescrito con el `derive` (validación) |
| + | `fmi-export/tests/test_scheduled_execution.rs` | Test de integración nuevo del camino FFI completo (incl. multi-reloj) |

> Los cambios de conformidad fina (tracking del *qualifier* **por-reloj**, `None →
> fmi3IntervalNotYetKnown`, y el `extern "C"` de `fmi3GetIntervalDecimal`) surgieron de una
> revisión adversarial contra el estándar; se explican en §3 y §4.

---

## 1. El problema, en concreto

### 1.1 El "pastel de capas" (imprescindible para entender lo demás)

Una FMU exportada con `rust-fmi` tiene varias capas. Es fácil confundir `UserModel`
con una macro; no lo es. Conviven **dos macros** y **varios traits**:

| Nombre | Qué es | Quién lo escribe |
|--------|--------|------------------|
| `#[derive(FmuModel)]` | macro *derive* | el usuario, sobre su struct |
| `export_fmu!(MiModelo)` | macro de función | el usuario, una vez |
| `UserModel` | **trait**: la física del modelo | **el usuario, a mano** |
| `Model` | trait: metadatos (nombre, nº estados, flags ME/CS/SE) | lo genera el *derive* |
| `ScheduledExecution` / `CoSimulation` / `ModelExchange` | traits: semántica FMI de cada interfaz, sobre `ModelInstance` | `fmi-export` |
| `Fmi3Common` / `Fmi3ScheduledExecution` / … | traits *wrapper*: ABI C crudo (`extern "C"`) | `fmi-export`, con impl por defecto |

**El flujo de una llamada** (activar una partición) atraviesa las capas así:

```
1. fmi3ActivateModelPartition(inst, clock_ref, t)                       ← extern "C", la genera export_fmu!
2.   <MiModelo as Fmi3ScheduledExecution>::fmi3_activate_model_partition ← wrapper ABI (wrappers.rs)
3.     <ModelInstance<M> as ScheduledExecution>::activate_model_partition ← semántica FMI (impl_se.rs)
4.       self.model.activate_partition(...)                              ← la física del usuario (UserModel)
```

- El paso 2 (dispatch ABI) **ya estaba hecho**.
- El paso 3 era un **stub** que devolvía `Error`.
- El paso 4 **no existía**: `UserModel` no tenía ningún método para "activar una
  partición" ni para "declarar el próximo intervalo".

### 1.2 Los stubs que había (estado inicial)

Verificado leyendo el árbol actual antes de tocar nada:

| Función (capa ABI, `wrappers.rs`) | Estado inicial | Riesgo |
|---|---|---|
| `fmi3_instantiate_scheduled_execution` | `todo!("…not yet implemented")` | **La FMU SE no se podía ni crear.** Pánico por FFI. |
| `fmi3_free_instance` (rama SE) | solo `eprintln!` — no liberaba el `Box` | Fuga de memoria + incoherente con instanciar |
| `fmi3_get_interval_decimal` | `todo!()` | Pánico por FFI (UB) |
| `fmi3_get_interval_fraction` | `todo!()` | Pánico por FFI (UB) |
| `fmi3_get_shift_decimal` / `_fraction` | `todo!()` | Pánico por FFI (UB) |
| `fmi3_set_interval_decimal` / `_fraction` | `todo!()` | Pánico por FFI (UB) |
| `fmi3_set_shift_decimal` / `_fraction` | `todo!()` | Pánico por FFI (UB) |
| `fmi3_evaluate_discrete_states` | `todo!()` | Pánico por FFI (UB) |
| `activate_model_partition` (`impl_se.rs`) | `Err(Error)` siempre | La partición nunca corría |

> **Hallazgo que `SE_PLAN.md` no listó:** `fmi3_instantiate_scheduled_execution` era un
> `todo!()`. Sin instanciación no se puede hacer nada; era el bloqueo nº 1. También la
> rama SE de `fmi3_free_instance` estaba rota (no liberaba). Ambos se arreglan aquí.

> **Nota sobre `todo!()` a través de FFI.** `todo!()` genera un `panic!`. Un `panic!`
> que cruza la frontera `extern "C"` hacia el importador (código C) es **comportamiento
> indefinido** en Rust. Es decir: cualquiera de esos stubs, si el importador lo llamaba,
> podía corromper el proceso. Convertirlos en un `return fmi3Error` honesto ya es, por sí
> solo, una corrección de seguridad, independientemente de si además se implementan.

---

## 2. Conceptos FMI 3.0 SE que hay que tener claros

Contrastado con el estándar FMI 3.0.1 (§ *FMI for Scheduled Execution* y § *Clocks*):

- En SE el modelo se divide en **particiones**, cada una atada a un **input clock**.
  El planificador del importador activa la partición con
  `fmi3ActivateModelPartition(clock_reference, activation_time)` cuando ese reloj hace
  *tick*. **`activate_model_partition` es a SE lo que `do_step` es a CS.**
- Un **countdown clock** es un *input clock* con `intervalVariability="countdown"` cuyo
  intervalo lo **decide la propia FMU**. Tras cada activación, el planificador lee
  `fmi3GetIntervalDecimal` para saber cuánto falta para el próximo *tick* (la σ) y
  reprograma la siguiente activación. Ese bucle
  `activar → getInterval → programar → activar …` **es** el mecanismo aperiódico (DEVS).
- **Qualifiers del intervalo** que devuelve `fmi3GetInterval*`:
  - `fmi3IntervalChanged` (2): hay un intervalo nuevo, su valor va en el buffer.
  - `fmi3IntervalUnchanged` (1): un `getInterval` previo ya devolvió el intervalo con
    `Changed` y **no ha cambiado desde entonces**.
  - `fmi3IntervalNotYetKnown` (0): aún no se conoce.
  - Regla del estándar: por *tick*, un intervalo reportado como `Changed` **no debe
    observarse dos veces** — una segunda lectura sin activación intermedia debe dar
    `Unchanged`.
- **Máquina de estados SE:** `Instantiated → InitializationMode → ClockActivationMode`
  (cíclico). Las particiones solo se activan en `ClockActivationMode`. `ModelState`
  ya tenía la variante `ClockActivationMode`, y una instancia SE **ya entraba** en ese
  estado al salir de inicialización (`common.rs`). No hizo falta ningún estado nuevo.

### El semáforo como átomo DEVS (mapeo directo)

| DEVS | FMI 3.0 SE | Hook `UserModel` |
|---|---|---|
| `ta()` = σ restante | `fmi3GetIntervalDecimal(reloj)` | `next_interval` |
| `δint + λ` (fin de fase) | `fmi3ActivateModelPartition` con σ agotada | `activate_partition` |
| `δext` (cambió una entrada) | `fmi3ActivateModelPartition` con σ>0 (el botón) | `activate_partition` |
| salidas = f(estado) | `fmi3GetFloat64` | `calculate_values` |

---

## 3. Diseño: las tres decisiones clave

### Decisión 1 — Dos hooks nuevos en `UserModel` (no reusar `calculate_values`)

`activate_model_partition` necesita ejecutar la física de *esa* partición, y los
countdown clocks necesitan que el modelo declare su próximo intervalo. Ninguna de las
dos cosas encajaba en los métodos existentes. Se añadieron **dos métodos con
implementación por defecto** (para no romper ningún modelo existente):

```rust
// activa la partición del reloj `clock_reference` en el instante `time`.
// default: recalcula todo el modelo (correcto para 1 sola partición).
fn activate_partition(&mut self, context, clock_reference, time) -> Result<Fmi3Res, Fmi3Error> {
    self.calculate_values(&*context)
}

// intervalo (s) hasta el próximo tick del countdown clock; None = sin countdown / sin cambio.
fn next_interval(&self, clock_reference) -> Option<f64> { None }
```

**Por qué así:** un modelo simple (una partición) no escribe nada; hereda los defaults.
Un modelo multi-partición sobreescribe `activate_partition` y hace
`match clock_reference { … }`. Es la traducción natural de `δ` y `ta()` de DEVS. El motor
(`MotorTermico`, que es CS) ni se entera: los defaults no afectan a CS/ME.

### Decisión 2 — El *qualifier* "observar una vez" se lleva en la **instancia y por-reloj**

Para cumplir la regla del estándar ("un `Changed` no se observa dos veces por *tick*")
sin ensuciar la API del usuario, `ModelInstance` lleva la contabilidad. La primera versión
usaba un único `bool` para toda la instancia; una revisión adversarial señaló (con razón)
que el estándar rastrea el *qualifier* **por reloj**: activar el reloj A no debe hacer que
el reloj B parezca `Changed`, y leer A no debe borrar el estado de B. Así que la versión
final usa un **conjunto de VRs de reloj ya observados** (`intervals_observed`):

- El usuario solo dice "mi intervalo actual del reloj `vr` es σ" (`next_interval(vr)`).
- La instancia decide el *qualifier*, por reloj:
  - `Some(σ)` y el reloj **no** estaba en el conjunto (primera lectura desde su *tick*) →
    `fmi3IntervalChanged`, y se inserta en el conjunto.
  - `Some(σ)` y ya estaba (relectura sin *tick*) → `fmi3IntervalUnchanged`.
  - `None` → `fmi3IntervalNotYetKnown` (ver Decisión 2b).
- `activate_model_partition(clock_ref)` **quita solo ese reloj** del conjunto (vuelve a ser
  `Changed`), sin tocar los demás.

Esto es **estrictamente conforme** también para modelos multi-partición, y produce para el
semáforo (un reloj, consultado una vez por *tick*) exactamente la misma traza que la
versión a mano. Se valida con el test `se_multiclock_qualifiers_are_per_clock`.

### Decisión 2b — `None` significa `fmi3IntervalNotYetKnown`, no `Unchanged`

`fmi3IntervalUnchanged` le dice al importador "sigue usando el intervalo anterior"; solo es
válido si **hubo** uno. Si el modelo aún no conoce el intervalo de un reloj
(`next_interval` → `None`), lo correcto es `fmi3IntervalNotYetKnown` (el importador debe
esperar, no reutilizar un valor viejo/basura). Con el tracking por-reloj, `Option<f64>` es
suficiente: la instancia distingue `Changed`/`Unchanged` con el conjunto, y `None` se mapea
a `NotYetKnown`. El usuario nunca necesita expresar `Unchanged` a mano.

### Decisión 3 — Las funciones `set_*` de reloj devuelven `fmi3Error` (honesto), no una implementación no verificada

`fmi3SetIntervalDecimal/Fraction` y `fmi3SetShiftDecimal/Fraction` sirven para que el
*entorno* fije el intervalo/shift de un reloj **tunable**. Ningún modelo actual expone un
reloj tunable, y nuestros relojes son *countdown* (definidos por la FMU, no por el
entorno). Implementar código `unsafe` de FFI **sin ningún modelo que lo ejercite** =
enviar código no verificable, justo donde se esconden las malinterpretaciones sutiles del
estándar. Un `fmi3Error` honesto es **más correcto** que una implementación
plausible-pero-no-probada. (Y, sobre todo, ya no es un `todo!()` que provoca UB.)

---

## 4. Implementación, archivo por archivo

### 4.1 `traits/mod.rs` — los hooks de `UserModel`

Se añaden `activate_partition` y `next_interval` al final del trait `UserModel`, con los
defaults de la Decisión 1 y documentación que enlaza al estándar. Import ya disponible:
`fmi::fmi3::binding` (para `fmi3ValueReference`).

### 4.2 `instance/mod.rs` — el estado del *qualifier* por-reloj

Se añade a `ModelInstance` el campo
`intervals_observed: HashSet<fmi3ValueReference>`, inicializado vacío en `new()` (todos los
relojes empiezan "pendientes" → su primera lectura es `Changed`, que es el intervalo
inicial). Ver Decisión 2 para la semántica.

> **Seguridad del `#[repr(C)]`:** la macro `dispatch_by_instance_type!` lee
> `(*ptr).instance_type` **a través de un puntero ya tipado** `*const ModelInstance<M,C>`,
> es decir, por *nombre de campo*, no por *offset numérico*. Añadir un campo (en cualquier
> posición salvo delante de nada que se lea por offset crudo) es seguro: `instance_type`
> sigue siendo el primer campo y se lee correctamente. No se rompe ninguna invariante.

### 4.3 `instance/common.rs` — `reset()`

`fmi3Reset` deja la FMU como recién creada. Se añade el reinicio de
`is_dirty_values = true` y `interval_changed_pending = true` para que el ciclo de vida
tras un reset sea idéntico al de una instancia nueva.

### 4.4 `instance/impl_se.rs` — el núcleo SE (archivo reescrito)

Contiene dos cosas:

**(a) `impl ScheduledExecution for ModelInstance` → `activate_model_partition`.**
Calcado de `do_step` de `impl_cs.rs`:
1. Traza + `assert_instance_type(ScheduledExecution)`.
2. Exige `state == ClockActivationMode` (si no, log + `Error`, **sin pánico**).
3. `context.set_time(activation_time)`.
4. `self.model.activate_partition(&mut self.context, clock_reference, activation_time)`.
5. `is_dirty_values = true` (las salidas se recalcularán al próximo `get`) y
   `intervals_observed.remove(&clock_reference)` (ese reloj hizo *tick* → su intervalo
   vuelve a ser `Changed`; los demás relojes no se tocan).

**(b) Métodos *inherentes* de intervalo/shift/discrete sobre `ModelInstance`**, a los que
los wrappers ABI hacen *dispatch*. Son inherentes (no de un trait) porque el trait
`ScheduledExecution` del crate `fmi` **solo** define `activate_model_partition`; las
funciones de intervalo no viven en ningún trait del núcleo, así que hay que crear el
"método de nivel superior" al que delegar. Son:

- `get_interval_decimal(clock_refs, intervals, qualifiers)`: por cada reloj consulta
  `model.next_interval(vr)`; si `Some(σ)` escribe σ y el *qualifier* según
  `intervals_observed` (ver Decisión 2: `insert(vr)` devuelve `true` → primera lectura →
  `Changed`; `false` → `Unchanged`); si `None` → `NotYetKnown`.
- `get_interval_fraction(...)`: lo mismo pero como racional `counter/resolution`, con
  `resolution = 1e9` (nanosegundos), suficientemente fino para no perder precisión con
  intervalos discretos.
- `get_shift_decimal` / `get_shift_fraction`: devuelven shift 0 (`0.0`, o `0/1`). Nuestros
  relojes no tienen desfase; 0 es el valor correcto de un reloj sin *shift*.
- `evaluate_discrete_states()`: recalcula si está *dirty* (mismo camino perezoso que los
  getters) y devuelve OK.

### 4.5 `traits/wrappers.rs` — la capa ABI C

**`fmi3_instantiate_scheduled_execution`** (era `todo!()`): calcado del de
Co-Simulation pero más simple (SE no tiene *early return* ni *intermediate update*).
Comprueba `SUPPORTS_SCHEDULED_EXECUTION` (si no, `null`), envuelve el callback de log,
crea `BasicContext` y `ModelInstance` con `InterfaceType::ScheduledExecution`, y devuelve
el `Box::into_raw`. Los callbacks de tiempo real (`clock_update`, `lock/unlock_preemption`)
se ignoran a propósito: los countdown clocks reportan su intervalo por *pull*
(`fmi3GetInterval*`), no por `clock_update`; y el *locking* de preempción es cosa de un
planificador preemptivo, no de la evaluación (mono-hilo) del modelo.

**`fmi3_free_instance` (rama SE)** (era solo `eprintln!`): ahora reconstruye el `Box` con
el **mismo tipo concreto** `ModelInstance<Self, BasicContext<Self>>` con que se creó y lo
libera (idéntico a la rama CS). Se elimina la fuga.

**Los 9 wrappers de reloj/discrete** (eran `todo!()`):
- `get_interval_decimal` / `get_interval_fraction` / `get_shift_decimal` /
  `get_shift_fraction`: construyen los *slices* de salida de longitud
  `n_value_references` y hacen *dispatch* al método de instancia. Manejan punteros nulos
  de forma defensiva (incluido el *out-param* opcional `qualifiers`, con un buffer
  temporal si viene nulo).
- `set_interval_*` / `set_shift_*`: `eprintln!` + `fmi3Error` (Decisión 3), **sin UB**.
- `evaluate_discrete_states`: *dispatch* a la instancia.

### 4.6 `export.rs` — `fmi3GetIntervalDecimal` debía ser `extern "C"` (bug de ABI)

El símbolo exportado `fmi3GetIntervalDecimal` estaba declarado como `unsafe fn` (ABI de
Rust) en lugar de `unsafe extern "C" fn`, a diferencia de su hermana
`fmi3GetIntervalFraction` y del resto de símbolos. Una función con ABI de Rust exportada
con `#[export_name]` y **llamada por un maestro C con convención C es comportamiento
indefinido** según el lenguaje (funciona "de casualidad" en x86-64 con argumentos escalares
/ punteros, por eso el test in-process en Rust no lo detecta — llama al método del trait, no
al símbolo C). Como `fmi3GetIntervalDecimal` es *la* función clave de un reloj *countdown*,
se corrige a `unsafe extern "C" fn`. (Los símbolos `fmi3*FMUState`, que comparten el mismo
patrón, siguen siendo `todo!()` fuera de alcance; ver §8.)

### 4.7 `variable_builder.rs` — propagar `clocks` (bug encontrado de paso)

El *builder* de variables solo copiaba el atributo `clocks` (que marca una variable como
"relojada") para las variables **binarias**; para `Float64`/`Int*`/`Bool` **lo ignoraba**,
aunque el struct del esquema sí tiene el campo `clocks`. Resultado: una salida relojada
`#[variable(..., clocks = [reloj])] rojo: f64` **no** emitía el atributo `clocks="…"` en el
XML. Se corrige en los `finish()` de float, int y bool (una línea cada uno). Sin esto, el
semáforo no podría declarar correctamente que sus salidas cambian en los *ticks* del reloj.

### 4.8 `semaforo_se` — reescrito con el `derive`

`Cargo.toml`: se añaden las dependencias `fmi` y `fmi-export` (antes: ninguna, ABI a mano).

`src/lib.rs`: el struct con `#[derive(FmuModel)]` y
`#[model(scheduled_execution = true, model_exchange = false, co_simulation = false, user_model = false)]`.
Variables anotadas: `boton` (input), `reloj` (`Clock`, `interval_variability = Countdown`),
`rojo`/`verde`/`t_restante` (outputs `clocks = [reloj]`). Los campos de estado interno
DEVS (`fase`, `sigma`, `elapsed`, `t_last`, `boton_atendido`) **no llevan atributo**, así
que el *derive* los ignora por completo (no salen en el XML ni en los get/set).

- `impl Default`: fija el estado inicial (ROJO, σ=60). Se hace en `Default` (y no en
  `configurate`) porque `fmi-export` ejecuta `calculate_values` al salir de
  Initialization Mode **antes** que `configurate`, así que el estado DEVS ya debe ser
  coherente al construir.
- `impl UserModel`: `calculate_values` (salidas ← estado), `activate_partition` (llama a
  `activar`, la lógica DEVS), `next_interval` (`Some(self.sigma)`).
- `export_fmu!(Semaforo)` genera todo el ABI C.

---

## 5. Antes / después del semáforo

| | Antes (a mano) | Ahora (derive) |
|---|---|---|
| Instanciación | `extern "C" fn fmi3InstantiateScheduledExecution` a mano | generado por `export_fmu!` |
| Activar partición | `extern "C" fn fmi3ActivateModelPartition` a mano | `UserModel::activate_partition` |
| Intervalo countdown | `extern "C" fn fmi3GetIntervalDecimal` a mano | `UserModel::next_interval` |
| Get/Set variables | `extern "C" fn fmi3Get/SetFloat64` a mano | generado por el `derive` |
| `modelDescription.xml` | escrito/mantenido aparte | **generado** por `cargo fmi` desde el struct |
| VRs | fijados a mano (0, 1000, 1001, 1002, 5000) | secuenciales (1..5); el importador lee el XML |
| Líneas de `unsafe` FFI | ~15 funciones `extern "C"` | 0 (las genera la librería) |

> **Sobre los VRs:** la versión a mano fijaba VRs arbitrarios (5000, 1000…). El *derive*
> asigna VRs secuenciales por orden de campo (1, 2, 3…), porque todo el *dispatch* de
> get/set depende de que los VRs sean densos y consecutivos. Un importador correcto lee
> los VRs del `modelDescription.xml` (que es justo lo que hay que hacer en FMI), así que
> esto no es una limitación real; solo hay que regenerar el XML del orquestador si antes
> tenía los VRs a fuego.

---

## 6. `modelDescription.xml` generado (real)

Producido con `cargo fmi bundle -p semaforo_se` + `cargo fmi inspect` sobre el `.fmu`:

```xml
<fmiModelDescription fmiVersion="3.0" modelName="semaforo_se" ...>
  <ScheduledExecution modelIdentifier="semaforo_se"/>
  <DefaultExperiment startTime="0" stopTime="180" stepSize="1"/>
  <ModelVariables>
    <Float64 name="time"       valueReference="0" causality="independent" variability="continuous"/>
    <Float64 name="boton"      valueReference="1" causality="input"  variability="discrete" start="0"/>
    <Clock   name="reloj"      valueReference="2" causality="input"  variability="discrete" intervalVariability="countdown"/>
    <Float64 name="rojo"       valueReference="3" causality="output" variability="discrete" clocks="2" start="1"/>
    <Float64 name="verde"      valueReference="4" causality="output" variability="discrete" clocks="2" start="0"/>
    <Float64 name="t_restante" valueReference="5" causality="output" variability="discrete" clocks="2" start="60"/>
  </ModelVariables>
  <ModelStructure>
    <Output valueReference="3"/><Output valueReference="4"/><Output valueReference="5"/>
  </ModelStructure>
</fmiModelDescription>
```

Puntos a comprobar (todos ✓):
- Solo interfaz `<ScheduledExecution>` (ni ME ni CS).
- `<Clock … causality="input" intervalVariability="countdown"/>` — reloj *countdown* de
  entrada, tal como manda el estándar para SE aperiódico.
- Las tres salidas llevan `clocks="2"` (relojadas por `reloj`) — prueba de que el arreglo
  del `variable_builder` (§4.7) funciona; antes ese atributo faltaba.

---

## 7. Validación

Todo compila y pasa. Comandos y resultados:

**Tests del núcleo `fmi-export` (incluye el test SE nuevo):**

```
cargo test -p fmi-export --features fmi3
```
- `test_scheduled_execution` → **4/4 OK**:
  - `se_full_lifecycle_through_c_abi`: instanciar SE → init → `getInterval`(=60, `Changed`)
    → `getInterval` otra vez (=`Unchanged`, regla "observar una vez") →
    `activate_model_partition(60)` → salidas (verde=1, rojo=0) → `getInterval`(=30,
    `Changed`) → otra activación → rojo, 60 → `evaluate_discrete_states` OK → `free`.
  - `se_interval_fraction_and_shift`: `getIntervalFraction` (60·1e9/1e9=60, `Changed`),
    `getShiftDecimal` (=0).
  - `se_multiclock_qualifiers_are_per_clock`: modelo con 3 relojes countdown; comprueba que
    activar el reloj A **no** pone a B en `Changed`, que leer A no borra B, y que un reloj
    sin intervalo da `NotYetKnown` (valida los arreglos de Decisión 2 y 2b, y el soporte
    multi-partición).
  - `se_activate_before_init_is_rejected`: activar fuera de `ClockActivationMode` →
    `Error` limpio (**no** pánico).
- Resto de tests del crate (variable_builder, array, child, dahlquist CS/ME, terminals):
  **todos OK, sin regresiones.**

**Tests del semáforo (lógica DEVS pura):**

```
cd semaforo_se && cargo test
```
→ **5/5 OK** (ciclo 60/30, botón temprano acorta a 15, botón tardío = cambio inmediato,
botón en verde se ignora, salidas reflejan estado).

**Tests del `derive` y build del motor (CS):** OK, sin regresiones.

**FMU real generado y XML inspeccionado:** ver §6.

---

## 8. Alcance, límites y trabajo futuro

**Implementado y verificado:** instanciar/liberar SE, `activate_model_partition`,
`get_interval_decimal`/`_fraction`, `get_shift_decimal`/`_fraction`,
`evaluate_discrete_states`, hooks `UserModel`, propagación de `clocks`, semáforo con
`derive`.

**Soportado (incluido multi-partición):** el tracking del *qualifier* es por-reloj, así
que **modelos con varias particiones/relojes countdown funcionan** (el usuario hace
`match clock_reference` en `activate_partition`/`next_interval`); validado con
`se_multiclock_qualifiers_are_per_clock`.

**Deliberadamente no implementado (con motivo):**
- `set_interval_*` / `set_shift_*` → `fmi3Error`. No hay modelo con reloj *tunable* que
  los ejercite; ver Decisión 3. Cuando exista uno, se implementan con test.
- Relojes con *shift* ≠ 0: `get_shift_*` devuelve 0 (correcto para relojes sin desfase).
  Un *shift* distinto de cero necesitaría almacenamiento por-reloj; ningún modelo actual lo
  usa (YAGNI).

**Fuera de alcance (no es SE ni exportación):**
- `fmi3GetFMUState`/`SetFMUState`/derivadas direccionales: siguen siendo `todo!()` en
  `wrappers.rs`. Son funciones **generales** (guardar/restaurar estado, sensibilidades),
  no específicas de SE, y son pre-existentes (el motor CS ya las traía así). No se tocaron
  para mantener el cambio enfocado en SE. Convertirlas a `fmi3Error` sería un *follow-up*
  trivial de seguridad.
- El lado **importador** del crate `fmi` (`fmi/src/fmi3/instance/scheduled_execution.rs`:
  `clock_update`, `lock_preemption`…): es para que `fmi-sim` *ejecute* FMUs SE. Este
  trabajo es el lado **exportador**. No se tocó.

---

## 9. Cómo reproducir / usar

**Escribir una FMU SE nueva** (patrón general):

```rust
#[derive(FmuModel, Debug)]
#[model(scheduled_execution = true, model_exchange = false, co_simulation = false, user_model = false)]
struct MiModelo {
    #[variable(causality = Input, interval_variability = Countdown)]
    reloj: Clock,
    #[variable(causality = Output, variability = Discrete, start = 0.0, clocks = [reloj])]
    salida: f64,
    // estado interno sin atributos → invisible para el derive
    sigma: f64,
}
impl UserModel for MiModelo {
    type LoggingCategory = DefaultLoggingCategory;
    fn calculate_values(&mut self, _c) -> Result<Fmi3Res, Fmi3Error> { /* salidas ← estado */ Ok(Fmi3Res::OK) }
    fn activate_partition(&mut self, _c, _clk, t) -> Result<Fmi3Res, Fmi3Error> { /* δ + λ */ Ok(Fmi3Res::OK) }
    fn next_interval(&self, _clk) -> Option<f64> { Some(self.sigma) } // ta()
}
fmi_export::export_fmu!(MiModelo);
```

**Compilar, probar, empaquetar el semáforo:**

```bash
cd semaforo_se
cargo test                                   # lógica DEVS
../rust-fmi/target/debug/cargo-fmi.exe bundle -p semaforo_se       # genera target/fmu/semaforo_se.fmu
../rust-fmi/target/debug/cargo-fmi.exe inspect target/fmu/semaforo_se.fmu --format model-description
```

**Ver el motor SE / correr el test de integración:**

```bash
cd rust-fmi
cargo test -p fmi-export --features fmi3 test_scheduled_execution -- --nocapture
```
