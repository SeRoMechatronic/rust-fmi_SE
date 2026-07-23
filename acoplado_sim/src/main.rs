//! CO-SIMULACIÓN ACOPLADA: generador de botón (Co-Simulation) + semáforo (Scheduled Execution).
//!
//! Es una **referencia ejecutable de lo que debe hacer SeRo_CoSim**. Conecta:
//!
//! ```text
//!   boton_pwm_cs.salida (VR 5)  ──►  semaforoV2_se.boton (VR 1)
//! ```
//!
//! # Por qué se cargan las FMUs como DLL (y no se enlazan)
//!
//! Cada FMU exporta los mismos símbolos C (`fmi3GetFloat64`, `fmi3Terminate`, …). Enlazar
//! dos FMUs en un mismo binario da error de "símbolo duplicado". Un orquestador real las
//! carga en tiempo de ejecución (`LoadLibrary`/`dlopen`), que es lo que se hace aquí con
//! `libloading`: cada DLL tiene su propio espacio de símbolos.
//!
//! # El punto crítico del acoplamiento CS ↔ SE
//!
//! El semáforo (SE) **solo reacciona cuando se activa su partición**. No basta con
//! escribirle `boton`: hay que llamar a `fmi3ActivateModelPartition` en el instante en que
//! la entrada cambia (δext). Si solo se activa al vencer el countdown, la pulsación no
//! tiene ningún efecto.
//!
//! Por eso el planificador fusiona **dos fuentes de eventos**:
//!   1. **δint (interno)**: vence el countdown → `fmi3GetIntervalDecimal` dice cuándo;
//!      se activa la partición y se vuelve a leer el intervalo.
//!   2. **δext (externo)**: la salida del generador CS cambia de flanco → se escribe
//!      `boton` y se activa la partición en ese mismo instante.
//!
//! El generador CS se avanza con `fmi3DoStep` en una rejilla fina (`H`) para detectar flancos.
//!
//! # Uso
//!
//! ```text
//! # 1) compilar las dos FMUs (generan las DLL en su target/debug)
//! cd ../boton_pwm_cs   && cargo build
//! cd ../semaforoV2_se  && cargo build
//! # 2) correr la co-simulación
//! cd ../acoplado_sim   && cargo run
//! ```

use std::ffi::{c_char, c_int, c_void, CString};
use std::path::PathBuf;

use libloading::{Library, Symbol};

// ── Tipos del ABI FMI 3.0 ─────────────────────────────────────────────────────
type Instance = *mut c_void;
type Status = c_int; // 0=OK, 1=Warning, 2=Discard, 3=Error, 4=Fatal
type ValueRef = u32;

type FnInstantiateCs = unsafe extern "C" fn(
    *const c_char, // instanceName
    *const c_char, // instantiationToken
    *const c_char, // resourcePath
    bool,          // visible
    bool,          // loggingOn
    bool,          // eventModeUsed
    bool,          // earlyReturnAllowed
    *const ValueRef,
    usize,
    *mut c_void, // instanceEnvironment
    *mut c_void, // logMessage
    *mut c_void, // intermediateUpdate
) -> Instance;

type FnInstantiateSe = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    *const c_char,
    bool,        // visible
    bool,        // loggingOn
    *mut c_void, // instanceEnvironment
    *mut c_void, // logMessage
    *mut c_void, // clockUpdate
    *mut c_void, // lockPreemption
    *mut c_void, // unlockPreemption
) -> Instance;

type FnEnterInit = unsafe extern "C" fn(Instance, bool, f64, f64, bool, f64) -> Status;
type FnExitInit = unsafe extern "C" fn(Instance) -> Status;
type FnDoStep = unsafe extern "C" fn(
    Instance,
    f64,
    f64,
    bool,
    *mut bool,
    *mut bool,
    *mut bool,
    *mut f64,
) -> Status;
type FnActivatePartition = unsafe extern "C" fn(Instance, ValueRef, f64) -> Status;
type FnGetFloat64 = unsafe extern "C" fn(Instance, *const ValueRef, usize, *mut f64, usize) -> Status;
type FnSetFloat64 =
    unsafe extern "C" fn(Instance, *const ValueRef, usize, *const f64, usize) -> Status;
type FnGetIntervalDecimal =
    unsafe extern "C" fn(Instance, *const ValueRef, usize, *mut f64, *mut c_int) -> Status;
type FnTerminate = unsafe extern "C" fn(Instance) -> Status;
type FnFreeInstance = unsafe extern "C" fn(Instance);

/// Una FMU cargada dinámicamente, con los punteros a función que necesitamos.
struct Fmu {
    _lib: Library, // debe seguir viva mientras usemos los punteros
    token: String,
    enter_init: FnEnterInit,
    exit_init: FnExitInit,
    get_f64: FnGetFloat64,
    set_f64: FnSetFloat64,
    terminate: FnTerminate,
    free_instance: FnFreeInstance,
}

/// Nombre de archivo de la biblioteca dinámica según el SO.
fn nombre_dll(crate_name: &str) -> String {
    #[cfg(target_os = "windows")]
    return format!("{crate_name}.dll");
    #[cfg(target_os = "macos")]
    return format!("lib{crate_name}.dylib");
    #[cfg(all(unix, not(target_os = "macos")))]
    return format!("lib{crate_name}.so");
}

impl Fmu {
    /// Carga la DLL de `<crate_dir>/target/debug/<lib>` y resuelve los símbolos comunes.
    unsafe fn cargar(crate_dir: &str, crate_name: &str) -> Fmu {
        let path: PathBuf = [crate_dir, "target", "debug", &nombre_dll(crate_name)]
            .iter()
            .collect();
        let lib = unsafe { Library::new(&path) }
            .unwrap_or_else(|e| panic!("No pude cargar {}: {e}\n¿Compilaste esa FMU con `cargo build`?", path.display()));

        // El token de instanciación: la FMU lo valida y si no coincide devuelve NULL.
        // Atajo válido aquí porque ambas FMUs son Rust: leemos el `&'static str` exportado.
        // (Un orquestador genérico lo leería del modelDescription.xml.)
        //
        // Ojo con el tipo: `libloading` reinterpreta la DIRECCIÓN del símbolo como `T`, así
        // que `T` debe medir lo que un puntero. Un `&str` es un puntero GORDO (dato+longitud,
        // 16 bytes) → `get::<&str>` falla con `IncompatibleSize`. Se pide `*const &str`
        // (puntero fino) y se desreferencia dos veces. Es el mismo patrón que usa cargo-fmi.
        let token: String = unsafe {
            let sym: Symbol<*const &str> = lib
                .get(b"FMI3_INSTANTIATION_TOKEN\0")
                .expect("falta FMI3_INSTANTIATION_TOKEN");
            (**sym).to_string()
        };

        unsafe {
            Fmu {
                enter_init: *lib
                    .get::<FnEnterInit>(b"fmi3EnterInitializationMode\0")
                    .unwrap(),
                exit_init: *lib
                    .get::<FnExitInit>(b"fmi3ExitInitializationMode\0")
                    .unwrap(),
                get_f64: *lib.get::<FnGetFloat64>(b"fmi3GetFloat64\0").unwrap(),
                set_f64: *lib.get::<FnSetFloat64>(b"fmi3SetFloat64\0").unwrap(),
                terminate: *lib.get::<FnTerminate>(b"fmi3Terminate\0").unwrap(),
                free_instance: *lib.get::<FnFreeInstance>(b"fmi3FreeInstance\0").unwrap(),
                token,
                _lib: lib,
            }
        }
    }

    unsafe fn get(&self, inst: Instance, vr: ValueRef) -> f64 {
        let mut v = [0.0f64; 1];
        let st = unsafe { (self.get_f64)(inst, [vr].as_ptr(), 1, v.as_mut_ptr(), 1) };
        assert_eq!(st, 0, "fmi3GetFloat64 falló");
        v[0]
    }

    unsafe fn set(&self, inst: Instance, vr: ValueRef, val: f64) {
        let st = unsafe { (self.set_f64)(inst, [vr].as_ptr(), 1, [val].as_ptr(), 1) };
        assert_eq!(st, 0, "fmi3SetFloat64 falló");
    }
}

// ── Value References (del modelDescription.xml de cada FMU) ───────────────────
// Generador (CS): time=0, t_inicio=1, periodo=2, ancho_pulso=3, amplitud=4, salida=5
const VR_GEN_T_INICIO: ValueRef = 1;
const VR_GEN_PERIODO: ValueRef = 2;
const VR_GEN_ANCHO: ValueRef = 3;
const VR_GEN_SALIDA: ValueRef = 5;
// Semáforo (SE): time=0, boton=1, reloj=2, rojo=3, verde=4, t_restante=5
const VR_SEM_BOTON: ValueRef = 1;
const VR_SEM_RELOJ: ValueRef = 2;
const VR_SEM_ROJO: ValueRef = 3;
const VR_SEM_VERDE: ValueRef = 4;

const H: f64 = 0.5; // paso de comunicación del generador CS [s]
const T_END: f64 = 130.0; // duración de la co-simulación [s]
const EPS: f64 = 1e-9;

fn fase(rojo: f64, verde: f64) -> &'static str {
    if rojo >= 0.5 {
        "🔴 ROJO "
    } else if verde >= 0.5 {
        "🟢 VERDE"
    } else {
        "  ?     "
    }
}

fn main() {
    unsafe {
        // ── 1. Cargar las dos FMUs ─────────────────────────────────────────────
        let gen_fmu = Fmu::cargar("../boton_pwm_cs", "boton_pwm_cs");
        let sem_fmu = Fmu::cargar("../semaforoV2_se", "semaforoV2_se");

        let instantiate_cs: FnInstantiateCs = *gen_fmu
            ._lib
            .get(b"fmi3InstantiateCoSimulation\0")
            .expect("fmi3InstantiateCoSimulation ausente");
        let do_step: FnDoStep = *gen_fmu._lib.get(b"fmi3DoStep\0").expect("fmi3DoStep ausente");

        let instantiate_se: FnInstantiateSe = *sem_fmu
            ._lib
            .get(b"fmi3InstantiateScheduledExecution\0")
            .expect("fmi3InstantiateScheduledExecution ausente");
        let activate: FnActivatePartition = *sem_fmu
            ._lib
            .get(b"fmi3ActivateModelPartition\0")
            .expect("fmi3ActivateModelPartition ausente");
        let get_interval: FnGetIntervalDecimal = *sem_fmu
            ._lib
            .get(b"fmi3GetIntervalDecimal\0")
            .expect("fmi3GetIntervalDecimal ausente");

        // ── 2. Instanciar ──────────────────────────────────────────────────────
        let nombre_gen = CString::new("generador").unwrap();
        let token_gen = CString::new(gen_fmu.token.as_str()).unwrap();
        let recurso = CString::new(".").unwrap();
        let gen = instantiate_cs(
            nombre_gen.as_ptr(),
            token_gen.as_ptr(),
            recurso.as_ptr(),
            false, // visible
            false, // loggingOn
            false, // eventModeUsed
            false, // earlyReturnAllowed
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert!(!gen.is_null(), "no se pudo instanciar el generador (CS)");

        let nombre_sem = CString::new("semaforo").unwrap();
        let token_sem = CString::new(sem_fmu.token.as_str()).unwrap();
        let sem = instantiate_se(
            nombre_sem.as_ptr(),
            token_sem.as_ptr(),
            recurso.as_ptr(),
            false,
            false,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert!(!sem.is_null(), "no se pudo instanciar el semáforo (SE)");

        // ── 3. Inicialización ──────────────────────────────────────────────────
        // Generador: pulso de 2 s cada 40 s, empezando a los 8 s (durante el primer rojo).
        (gen_fmu.enter_init)(gen, false, 0.0, 0.0, true, T_END);
        gen_fmu.set(gen, VR_GEN_T_INICIO, 8.0);
        gen_fmu.set(gen, VR_GEN_PERIODO, 40.0);
        gen_fmu.set(gen, VR_GEN_ANCHO, 2.0);
        (gen_fmu.exit_init)(gen);

        (sem_fmu.enter_init)(sem, false, 0.0, 0.0, true, T_END);
        (sem_fmu.exit_init)(sem);

        // Lee σ del reloj countdown del semáforo.
        let leer_sigma = || -> f64 {
            let mut interval = [0.0f64; 1];
            let mut qual = [0 as c_int; 1];
            let st = get_interval(
                sem,
                [VR_SEM_RELOJ].as_ptr(),
                1,
                interval.as_mut_ptr(),
                qual.as_mut_ptr(),
            );
            assert_eq!(st, 0, "fmi3GetIntervalDecimal falló");
            interval[0]
        };

        // ── 4. Bucle del planificador (fusiona eventos internos y externos) ────
        let mut t = 0.0f64;
        let mut sigma = leer_sigma();
        let mut t_reloj = t + sigma; // próximo evento interno del semáforo
        let mut t_rejilla = H; // próximo punto de comunicación del CS
        let mut boton_prev = 0.0f64;

        println!("  t (s) | fase    |  σ (s) | botón | evento");
        println!("  ------+---------+--------+-------+------------------------------");
        println!(
            "  {t:>5.1} | {} | {sigma:>6.1} |  {boton_prev:>3.0}  | inicio",
            fase(sem_fmu.get(sem, VR_SEM_ROJO), sem_fmu.get(sem, VR_SEM_VERDE))
        );

        while t < T_END - EPS {
            // Próximo instante de interés: la rejilla del CS o el vencimiento del reloj.
            let t_next = t_rejilla.min(t_reloj).min(T_END);

            // (a) Avanzar el generador CS hasta t_next.
            let h = t_next - t;
            if h > EPS {
                let st = do_step(
                    gen,
                    t,
                    h,
                    false,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                assert_eq!(st, 0, "fmi3DoStep del generador falló");
            }
            t = t_next;
            let boton = gen_fmu.get(gen, VR_GEN_SALIDA);

            let mut evento: Option<&str> = None;

            // (b) ¿Venció el countdown? → evento INTERNO (δint).
            if (t - t_reloj).abs() < EPS {
                assert_eq!(activate(sem, VR_SEM_RELOJ, t), 0, "activate (δint) falló");
                sigma = leer_sigma();
                t_reloj = t + sigma;
                evento = Some("δint: fin de fase");
            }

            // (c) ¿Cambió el botón? → evento EXTERNO (δext). ¡Esto es lo imprescindible!
            if (boton - boton_prev).abs() > 0.5 {
                sem_fmu.set(sem, VR_SEM_BOTON, boton);
                assert_eq!(activate(sem, VR_SEM_RELOJ, t), 0, "activate (δext) falló");
                sigma = leer_sigma();
                t_reloj = t + sigma;
                evento = Some(if boton >= 0.5 {
                    "δext: 👆 BOTÓN pulsado"
                } else {
                    "δext: botón soltado"
                });
                boton_prev = boton;
            }

            // (d) Avanzar la rejilla del CS si tocaba.
            if (t - t_rejilla).abs() < EPS {
                t_rejilla += H;
            }

            // Solo imprimimos cuando pasa algo (si no, serían cientos de filas).
            if let Some(ev) = evento {
                println!(
                    "  {t:>5.1} | {} | {sigma:>6.1} |  {boton:>3.0}  | {ev}",
                    fase(sem_fmu.get(sem, VR_SEM_ROJO), sem_fmu.get(sem, VR_SEM_VERDE))
                );
            }
        }

        // ── 5. Terminar y liberar ──────────────────────────────────────────────
        (gen_fmu.terminate)(gen);
        (sem_fmu.terminate)(sem);
        (gen_fmu.free_instance)(gen);
        (sem_fmu.free_instance)(sem);
        println!("\n  Fin de la co-simulación ({T_END} s).");
    }
}
