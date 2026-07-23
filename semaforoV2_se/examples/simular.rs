//! Mini-planificador (scheduler) de FMI 3.0 Scheduled Execution para el semáforo V2.
//!
//! ¿Por qué esto y no `fmi-sim`? El simulador de rust-fmi (`fmi-sim`) sí ejecuta
//! Co-Simulation y Model Exchange, pero su rama de Scheduled Execution es
//! `unimplemented!()` (fmi-sim/src/sim/mod.rs). Un FMU de SE lo mueve un *scheduler*;
//! este ejemplo es un scheduler mínimo que:
//!
//!   1. instancia el FMU (interfaz SE),
//!   2. lee el intervalo del reloj countdown  → `fmi3GetIntervalDecimal`  (el ta() de DEVS),
//!   3. programa la siguiente activación en `t + σ`,
//!   4. activa la partición                    → `fmi3ActivateModelPartition`,
//!   5. lee las salidas                        → `fmi3GetFloat64`,
//!   y repite. También inyecta una pulsación de botón (δext) en un instante dado.
//!
//! Llama al FMU **por su ABI C real** (las mismas funciones que usaría SeRo_CoSim),
//! así que esto valida el FMU de verdad, no solo la lógica.
//!
//! Ejecutar:  cargo run --example simular

use std::ffi::CString;

use fmi::{
    fmi3::{binding, Fmi3Res, Fmi3Status},
    traits::FmiStatus,
};
use fmi_export::fmi3::{Fmi3Common, Fmi3ScheduledExecution, Model};
use semaforoV2_se::SemaforoV2;

// Value References (tal como salen en el modelDescription.xml)
const VR_BOTON: binding::fmi3ValueReference = 1;
const VR_RELOJ: binding::fmi3ValueReference = 2;
const VR_ROJO: binding::fmi3ValueReference = 3;
const VR_VERDE: binding::fmi3ValueReference = 4;

// ── Pequeños ayudantes sobre el ABI C ──────────────────────────────────────────

unsafe fn get_f64(inst: binding::fmi3Instance, vr: binding::fmi3ValueReference) -> f64 {
    let mut v = [0.0f64; 1];
    let st = Fmi3Status::from(unsafe {
        <SemaforoV2 as Fmi3Common>::fmi3_get_float64(inst, [vr].as_ptr(), 1, v.as_mut_ptr(), 1)
    })
    .ok();
    assert_eq!(st, Ok(Fmi3Res::OK), "fmi3GetFloat64 falló");
    v[0]
}

unsafe fn set_f64(inst: binding::fmi3Instance, vr: binding::fmi3ValueReference, val: f64) {
    let st = Fmi3Status::from(unsafe {
        <SemaforoV2 as Fmi3Common>::fmi3_set_float64(inst, [vr].as_ptr(), 1, [val].as_ptr(), 1)
    })
    .ok();
    assert_eq!(st, Ok(Fmi3Res::OK), "fmi3SetFloat64 falló");
}

/// Lee el intervalo del reloj countdown (el σ = ta() de DEVS).
unsafe fn get_interval(inst: binding::fmi3Instance) -> f64 {
    let mut interval = [0.0f64; 1];
    let mut qual = [0 as binding::fmi3IntervalQualifier; 1];
    let st = Fmi3Status::from(unsafe {
        <SemaforoV2 as Fmi3Common>::fmi3_get_interval_decimal(
            inst,
            [VR_RELOJ].as_ptr(),
            1,
            interval.as_mut_ptr(),
            qual.as_mut_ptr(),
        )
    })
    .ok();
    assert_eq!(st, Ok(Fmi3Res::OK), "fmi3GetIntervalDecimal falló");
    interval[0]
}

unsafe fn activar_particion(inst: binding::fmi3Instance, t: f64) {
    let st = Fmi3Status::from(unsafe {
        <SemaforoV2 as Fmi3ScheduledExecution>::fmi3_activate_model_partition(inst, VR_RELOJ, t)
    })
    .ok();
    assert_eq!(st, Ok(Fmi3Res::OK), "fmi3ActivateModelPartition falló");
}

fn estado(rojo: f64, verde: f64) -> &'static str {
    if rojo >= 0.5 {
        "🔴 ROJO "
    } else if verde >= 0.5 {
        "🟢 VERDE"
    } else {
        "  ?    "
    }
}

fn main() {
    unsafe {
        // ── 1. Instanciar la FMU (interfaz Scheduled Execution) ────────────────
        let inst = <SemaforoV2 as Fmi3Common>::fmi3_instantiate_scheduled_execution(
            CString::new("semaforo").unwrap().as_ptr(),
            CString::new(SemaforoV2::INSTANTIATION_TOKEN).unwrap().as_ptr() as *mut i8,
            CString::new(".").unwrap().as_ptr(),
            false as _, // visible
            false as _, // logging_on
            std::ptr::null_mut(),
            None, // log_message
            None, // clock_update
            None, // lock_preemption
            None, // unlock_preemption
        );
        assert!(!inst.is_null(), "no se pudo instanciar la FMU");

        // ── 2. Inicialización ──────────────────────────────────────────────────
        <SemaforoV2 as Fmi3Common>::fmi3_enter_initialization_mode(inst, false, 0.0, 0.0, true, 200.0);
        <SemaforoV2 as Fmi3Common>::fmi3_exit_initialization_mode(inst);

        // ── 3. Bucle del planificador ──────────────────────────────────────────
        let t_end = 80.0; // segundos a simular
        let boton_en = 10.0; // el peatón pulsa el botón a los 10 s (durante el primer rojo)
        let mut boton_pulsado = false;
        let mut t = 0.0;

        println!("  t (s) | estado  |  σ (s) hasta el próximo cambio");
        println!("  ------+---------+-------------------------------");
        loop {
            // Leemos y mostramos el estado en el instante t.
            let rojo = get_f64(inst, VR_ROJO);
            let verde = get_f64(inst, VR_VERDE);
            let sigma = get_interval(inst); // ta()
            println!("  {t:>5.1} | {} |  {sigma:>5.1}", estado(rojo, verde));

            if t >= t_end {
                break;
            }

            let t_evento_interno = t + sigma; // cuándo se agotaría la fase

            // ¿Toca la pulsación del botón antes del próximo evento interno?
            if !boton_pulsado && boton_en > t && boton_en <= t_evento_interno {
                set_f64(inst, VR_BOTON, 1.0); // pulsar
                activar_particion(inst, boton_en); // δext: activación por entrada
                set_f64(inst, VR_BOTON, 0.0); // soltar (botón momentáneo)
                boton_pulsado = true;
                t = boton_en;
                println!("        |  👆 botón pulsado → el rojo se acorta a 20 s en total");
            } else {
                activar_particion(inst, t_evento_interno); // δint: fin de fase
                t = t_evento_interno;
            }
        }

        // ── 4. Liberar ─────────────────────────────────────────────────────────
        <SemaforoV2 as Fmi3Common>::fmi3_free_instance(inst);
        println!("\n  Fin de la simulación ({t_end} s).");
    }
}
