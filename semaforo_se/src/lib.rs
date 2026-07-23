//! SEMÁFORO APERIÓDICO — FMU FMI 3.0 **Scheduled Execution** generada con `fmi-export`.
//!
//! Comportamiento (un átomo DEVS clásico):
//!   · ROJO 60 s → VERDE 30 s → ROJO 60 s → …
//!   · Si llega el BOTÓN durante el rojo, el rojo pasa a valer 15 s EN TOTAL:
//!     si ya pasaron ≥15 s de rojo, cambia a verde INMEDIATAMENTE; si no,
//!     cambia cuando se cumplan los 15.
//!
//! # De ABI-a-mano a `#[derive(FmuModel)]`
//!
//! La versión anterior de este archivo implementaba el ABI FMI 3.0 SE **a mano**
//! (`extern "C"` para `fmi3InstantiateScheduledExecution`, `fmi3ActivateModelPartition`,
//! `fmi3GetIntervalDecimal`, …) porque el MOTOR de Scheduled Execution de `fmi-export`
//! estaba sin implementar (`activate_model_partition` devolvía `Error`,
//! `fmi3GetIntervalDecimal` era un `todo!()` que provocaba pánico a través de FFI).
//!
//! Ahora ese motor **sí está implementado**, así que el semáforo se escribe igual que
//! cualquier otro modelo `fmi-export` (igual que el motor térmico en Co-Simulation):
//! se declara el struct con `#[derive(FmuModel)]`, se implementa `UserModel` a mano
//! (la física DEVS) y `export_fmu!` genera todo el ABI C. Ver `SE_IMPLEMENTACION.md`.
//!
//! # Mapeo DEVS → FMI 3.0 SE
//!
//! | DEVS                        | FMI 3.0 Scheduled Execution                         | Hook `UserModel`      |
//! |-----------------------------|-----------------------------------------------------|-----------------------|
//! | `ta()` = σ restante         | `fmi3GetIntervalDecimal(reloj)`                     | `next_interval`       |
//! | `δint + λ` (fin de fase)    | `fmi3ActivateModelPartition` con σ agotada          | `activate_partition`  |
//! | `δext` (cambió una entrada) | `fmi3ActivateModelPartition` con σ>0 (botón)        | `activate_partition`  |
//! | salidas = f(estado)         | `fmi3GetFloat64`                                     | `calculate_values`    |
//!
//! El reloj `reloj` es un **input clock** con `intervalVariability = "countdown"`: la
//! FMU decide cuándo es su próximo tick y el planificador lo lee tras cada activación.

use fmi::fmi3::{binding::fmi3ValueReference, Fmi3Error, Fmi3Res};
use fmi_export::{
    fmi3::{Clock, Context, DefaultLoggingCategory, UserModel},
    FmuModel,
};

// ── Constantes del modelo ─────────────────────────────────────────────────────
const ROJO_S: f64 = 60.0; // duración normal del rojo
const VERDE_S: f64 = 30.0; // duración del verde
const ROJO_CON_BOTON_S: f64 = 15.0; // duración TOTAL del rojo si hay botón
const EPS: f64 = 1e-9;

/// Fase del semáforo (estado discreto del átomo DEVS).
#[derive(PartialEq, Clone, Copy, Debug, Default)]
enum Fase {
    #[default]
    Rojo,
    Verde,
}

/// El semáforo como modelo `fmi-export`.
///
/// Las variables anotadas con `#[variable(...)]` forman la interfaz FMI (entradas,
/// salidas y el reloj). Los campos **sin anotación** (`fase`, `sigma`, …) son estado
/// interno del átomo DEVS: el derive los ignora por completo (no aparecen en el
/// `modelDescription.xml` ni en los get/set).
#[derive(FmuModel, Debug)]
#[model(
    model_exchange = false,
    co_simulation = false,
    scheduled_execution = true,
    user_model = false
)]
struct Semaforo {
    /// Botón peatonal — entrada (>=0.5 = pulsado).
    #[variable(causality = Input, variability = Discrete, start = 0.0)]
    boton: f64,

    /// Reloj *countdown* que dispara la partición del semáforo — input clock.
    /// La FMU declara "mi próximo tick es dentro de σ" vía `next_interval`.
    #[variable(causality = Input, interval_variability = Countdown)]
    reloj: Clock,

    /// Fase roja activa (1.0/0.0) — salida, cambia solo en los ticks de `reloj`.
    #[variable(causality = Output, variability = Discrete, start = 1.0, clocks = [reloj])]
    rojo: f64,

    /// Fase verde activa (1.0/0.0) — salida.
    #[variable(causality = Output, variability = Discrete, start = 0.0, clocks = [reloj])]
    verde: f64,

    /// Segundos hasta el próximo cambio de fase (σ) — salida.
    #[variable(causality = Output, variability = Discrete, start = 60.0, clocks = [reloj])]
    t_restante: f64,

    // ── Estado interno DEVS (sin atributos → invisible para el derive) ─────────
    /// Fase actual.
    fase: Fase,
    /// σ = ta(): tiempo que queda para el cambio de fase interno.
    sigma: f64,
    /// Tiempo transcurrido DENTRO de la fase actual.
    elapsed: f64,
    /// Instante de la última activación (para calcular el `e` de DEVS).
    t_last: f64,
    /// El acortamiento por botón se aplica UNA vez por fase roja.
    boton_atendido: bool,
}

impl Default for Semaforo {
    /// Estado inicial: ROJO recién empezado, σ = 60 s.
    ///
    /// Se inicializa aquí (y no en `configurate`) porque `fmi-export` ejecuta
    /// `calculate_values` al salir de Initialization Mode ANTES de `configurate`,
    /// así que el estado DEVS ya debe ser coherente en el momento de construir.
    fn default() -> Self {
        Self {
            boton: 0.0,
            reloj: Clock::default(),
            rojo: 1.0,
            verde: 0.0,
            t_restante: ROJO_S,
            fase: Fase::Rojo,
            sigma: ROJO_S,
            elapsed: 0.0,
            t_last: 0.0,
            boton_atendido: false,
        }
    }
}

impl Semaforo {
    /// Una activación de la partición en el instante `t` (interna o externa — se
    /// distingue por si σ se agotó, exactamente como en DEVS).
    fn activar(&mut self, t: f64) {
        // e = tiempo transcurrido desde la última activación.
        let e = (t - self.t_last).max(0.0);
        self.t_last = t;
        self.sigma = (self.sigma - e).max(0.0);
        self.elapsed += e;

        if self.sigma <= EPS {
            // ── δint + λ: se agotó la fase → cambiar ────────────────────────────
            match self.fase {
                Fase::Rojo => {
                    self.fase = Fase::Verde;
                    self.sigma = VERDE_S;
                }
                Fase::Verde => {
                    self.fase = Fase::Rojo;
                    self.sigma = ROJO_S;
                }
            }
            self.elapsed = 0.0;
            self.boton_atendido = false;
        } else {
            // ── δext: activación por cambio de entrada (el botón) ───────────────
            if self.fase == Fase::Rojo && self.boton >= 0.5 && !self.boton_atendido {
                self.boton_atendido = true;
                // El rojo pasa a valer 15 s EN TOTAL: si ya pasaron ≥15,
                // σ=0 → el planificador re-activa en este mismo instante y la
                // rama δint hace el cambio a verde "inmediato".
                self.sigma = (ROJO_CON_BOTON_S - self.elapsed).max(0.0);
            }
            // (botón durante verde: se ignora — cruce ya concedido)
        }
    }
}

impl UserModel for Semaforo {
    type LoggingCategory = DefaultLoggingCategory;

    /// Calcula las salidas a partir del estado DEVS (fase, σ). `fmi-export` la llama
    /// perezosamente cuando el maestro lee salidas tras una activación.
    fn calculate_values(&mut self, _context: &dyn Context<Self>) -> Result<Fmi3Res, Fmi3Error> {
        self.rojo = if self.fase == Fase::Rojo { 1.0 } else { 0.0 };
        self.verde = if self.fase == Fase::Verde { 1.0 } else { 0.0 };
        self.t_restante = self.sigma;
        Ok(Fmi3Res::OK)
    }

    /// `fmi3ActivateModelPartition`: ejecuta δint/δext + λ del átomo. Como el
    /// semáforo tiene una sola partición, el `clock_reference` es inequívoco y se
    /// ignora (un modelo multi-partición haría `match clock_reference { … }`).
    fn activate_partition(
        &mut self,
        _context: &mut dyn Context<Self>,
        _clock_reference: fmi3ValueReference,
        time: f64,
    ) -> Result<Fmi3Res, Fmi3Error> {
        self.activar(time);
        Ok(Fmi3Res::OK)
    }

    /// `ta()` de DEVS: el próximo tick del reloj countdown es dentro de σ segundos.
    /// `Some(σ)` → `fmi3IntervalChanged`.
    fn next_interval(&self, _clock_reference: fmi3ValueReference) -> Option<f64> {
        Some(self.sigma)
    }
}

fmi_export::export_fmu!(Semaforo);

// ── Tests del átomo DEVS (la lógica pura, sin FFI) ────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    /// Un `Context` mínimo no hace falta: probamos `activar`/`calculate_values`
    /// directamente sobre el estado.
    #[test]
    fn ciclo_normal_60_30() {
        let mut s = Semaforo::default();
        assert_eq!(s.fase, Fase::Rojo);
        s.activar(60.0); // tick interno: fin del rojo
        assert_eq!(s.fase, Fase::Verde);
        assert!((s.sigma - 30.0).abs() < 1e-9);
        s.activar(90.0); // fin del verde
        assert_eq!(s.fase, Fase::Rojo);
        assert!((s.sigma - 60.0).abs() < 1e-9);
    }

    #[test]
    fn boton_temprano_acorta_a_15() {
        let mut s = Semaforo::default();
        s.boton = 1.0;
        s.activar(10.0); // δext en t=10 (elapsed 10 < 15)
        assert_eq!(s.fase, Fase::Rojo);
        assert!((s.sigma - 5.0).abs() < 1e-9, "quedan 15-10=5 s de rojo");
        s.activar(15.0); // tick interno reprogramado
        assert_eq!(s.fase, Fase::Verde);
    }

    #[test]
    fn boton_tardio_cambia_inmediato() {
        let mut s = Semaforo::default();
        s.activar(50.0); // δext "vacía" (sin botón): solo avanza el tiempo
        s.boton = 1.0;
        s.activar(50.0); // δext con botón en t=50 (elapsed 50 ≥ 15)
        assert!(s.sigma <= EPS, "σ=0 → cambio inmediato pendiente");
        s.activar(50.0); // re-activación en el mismo instante → δint
        assert_eq!(s.fase, Fase::Verde);
    }

    #[test]
    fn boton_en_verde_se_ignora() {
        let mut s = Semaforo::default();
        s.activar(60.0); // → verde
        s.boton = 1.0;
        s.activar(70.0); // δext durante el verde
        assert_eq!(s.fase, Fase::Verde);
        assert!((s.sigma - 20.0).abs() < 1e-9, "el verde sigue su curso");
    }

    /// Las salidas reflejan el estado tras `calculate_values`.
    #[test]
    fn salidas_reflejan_estado() {
        let mut s = Semaforo::default();
        let ctx = TestCtx;
        s.calculate_values(&ctx).unwrap();
        assert_eq!(s.rojo, 1.0);
        assert_eq!(s.verde, 0.0);
        assert!((s.t_restante - 60.0).abs() < 1e-9);
        s.activar(60.0);
        s.calculate_values(&ctx).unwrap();
        assert_eq!(s.rojo, 0.0);
        assert_eq!(s.verde, 1.0);
        assert!((s.t_restante - 30.0).abs() < 1e-9);
    }

    // `calculate_values` solo necesita un `&dyn Context` que no usa; un stub basta.
    struct TestCtx;
    impl Context<Semaforo> for TestCtx {
        fn logging_on(&self, _c: DefaultLoggingCategory) -> bool {
            false
        }
        fn set_logging(&mut self, _c: DefaultLoggingCategory, _e: bool) {}
        fn log(
            &self,
            _s: fmi::fmi3::Fmi3Status,
            _c: DefaultLoggingCategory,
            _a: std::fmt::Arguments<'_>,
        ) {
        }
        fn resource_path(&self) -> &std::path::PathBuf {
            static P: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
            P.get_or_init(std::path::PathBuf::new)
        }
        fn initialize(&mut self, _start: f64, _stop: Option<f64>) {}
        fn time(&self) -> f64 {
            0.0
        }
        fn set_time(&mut self, _t: f64) {}
        fn stop_time(&self) -> Option<f64> {
            None
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }
}
