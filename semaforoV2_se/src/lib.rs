//! SEMÁFORO V2 — FMI 3.0 Scheduled Execution (reloj countdown), con fmi-export.
//!
//! Comportamiento (átomo DEVS):
//!   · ROJO 30 s → VERDE 15 s → ROJO 30 s → …
//!   · Si se pulsa el BOTÓN durante el rojo, el rojo pasa a valer 20 s EN TOTAL.
//!
//! (Se construye paso a paso; ver el chat.)

use fmi::fmi3::{binding::fmi3ValueReference, Fmi3Error, Fmi3Res};
use fmi_export::{
    fmi3::{Clock, Context, DefaultLoggingCategory, UserModel},
    FmuModel,
};

// ── Paso 1: parámetros del modelo ─────────────────────────────────────────────
const ROJO_S: f64 = 30.0; // duración normal del rojo
const VERDE_S: f64 = 15.0; // duración del verde
const ROJO_CON_BOTON_S: f64 = 20.0; // duración TOTAL del rojo si hay botón
const EPS: f64 = 1e-9; // tolerancia para "σ agotada"

/// Fase del semáforo (estado discreto del átomo DEVS).
#[derive(PartialEq, Clone, Copy, Debug, Default)]
enum Fase {
    #[default]
    Rojo,
    Verde,
}

// ── Paso 1: el struct = la interfaz FMI + el estado interno ────────────────────
#[derive(FmuModel, Debug)]
#[model(
    model_exchange = false,
    co_simulation = false,
    scheduled_execution = true,
    user_model = false
)]
pub struct SemaforoV2 {
    /// Botón peatonal — ENTRADA (>=0.5 = pulsado).
    #[variable(causality = Input, variability = Discrete, start = 0.0)]
    boton: f64,

    /// Reloj countdown que dispara la partición — INPUT CLOCK.
    #[variable(causality = Input, interval_variability = Countdown)]
    reloj: Clock,

    /// Rojo activo (1.0/0.0) — SALIDA, cambia en los ticks de `reloj`.
    #[variable(causality = Output, variability = Discrete, start = 1.0, clocks = [reloj])]
    rojo: f64,

    /// Verde activo (1.0/0.0) — SALIDA.
    #[variable(causality = Output, variability = Discrete, start = 0.0, clocks = [reloj])]
    verde: f64,

    /// Segundos hasta el próximo cambio de fase (σ) — SALIDA.
    #[variable(causality = Output, variability = Discrete, start = 30.0, clocks = [reloj])]
    t_restante: f64,

    // ── Estado interno DEVS (SIN atributos → invisible para el derive) ─────────
    fase: Fase,           // fase actual
    sigma: f64,           // σ = ta(): tiempo hasta el próximo cambio interno
    elapsed: f64,         // tiempo transcurrido dentro de la fase actual
    t_last: f64,          // instante de la última activación (para calcular e)
    boton_atendido: bool, // el acortamiento por botón se aplica 1 vez por fase roja
}

// ── Paso 2: estado inicial ─────────────────────────────────────────────────────
impl Default for SemaforoV2 {
    /// Estado inicial: ROJO recién empezado, σ = 30 s.
    ///
    /// Se hace en `Default` (y NO en `configurate`) porque fmi-export ejecuta
    /// `calculate_values` al salir de Initialization Mode ANTES de `configurate`;
    /// así, cuando se calculan las salidas por primera vez, el estado DEVS ya es
    /// coherente.
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

// ── Paso 3: la lógica DEVS ─────────────────────────────────────────────────────
impl SemaforoV2 {
    /// Una activación de la partición en el instante `t`.
    ///
    /// Distinguimos, como en DEVS, si es una transición interna (σ agotada → cambio de
    /// fase) o externa (llegó una entrada, el botón, con σ>0).
    fn activar(&mut self, t: f64) {
        // e = tiempo transcurrido desde la última activación.
        let e = (t - self.t_last).max(0.0);
        self.t_last = t;
        self.sigma = (self.sigma - e).max(0.0); // consumimos σ
        self.elapsed += e; // y avanzamos el tiempo dentro de la fase

        if self.sigma <= EPS {
            // ── δint + λ: se agotó la fase → cambiar ────────────────────────────
            match self.fase {
                Fase::Rojo => {
                    self.fase = Fase::Verde;
                    self.sigma = VERDE_S; // 15 s de verde
                }
                Fase::Verde => {
                    self.fase = Fase::Rojo;
                    self.sigma = ROJO_S; // 30 s de rojo
                }
            }
            self.elapsed = 0.0;
            self.boton_atendido = false; // el botón vuelve a poder acortar el nuevo rojo
        } else {
            // ── δext: activación por cambio de entrada (el botón) ───────────────
            if self.fase == Fase::Rojo && self.boton >= 0.5 && !self.boton_atendido {
                self.boton_atendido = true;
                // El rojo pasa a valer 20 s EN TOTAL: lo que quede es 20 − lo ya transcurrido.
                // Si ya pasaron ≥20 s, σ=0 → el planificador re-activa ya y la rama δint
                // hace el cambio a verde "inmediato".
                self.sigma = (ROJO_CON_BOTON_S - self.elapsed).max(0.0);
            }
            // (botón durante verde: se ignora — el cruce ya está concedido)
        }
    }
}

// ── Paso 4: la física, conectada al motor SE ───────────────────────────────────
impl UserModel for SemaforoV2 {
    type LoggingCategory = DefaultLoggingCategory;

    /// Calcula las SALIDAS a partir del estado DEVS (fase, σ). El motor la llama
    /// perezosamente cuando el maestro lee salidas tras una activación.
    fn calculate_values(&mut self, _context: &dyn Context<Self>) -> Result<Fmi3Res, Fmi3Error> {
        self.rojo = if self.fase == Fase::Rojo { 1.0 } else { 0.0 };
        self.verde = if self.fase == Fase::Verde { 1.0 } else { 0.0 };
        self.t_restante = self.sigma;
        Ok(Fmi3Res::OK)
    }

    /// `fmi3ActivateModelPartition`: ejecuta un tick de la partición (δint/δext + λ).
    /// Como el semáforo tiene UNA sola partición, `clock_reference` es inequívoco y se
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
    /// `Some(σ)` → el motor lo reporta como `fmi3IntervalChanged` la primera vez tras
    /// cada tick, y `fmi3IntervalUnchanged` en relecturas (lo gestiona la instancia).
    fn next_interval(&self, _clock_reference: fmi3ValueReference) -> Option<f64> {
        Some(self.sigma)
    }
}

// ── Paso 5: generar todo el ABI C de la FMU ────────────────────────────────────
fmi_export::export_fmu!(SemaforoV2);

// ── Paso 5: tests de la lógica DEVS (sin FFI) ──────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciclo_normal_30_15() {
        let mut s = SemaforoV2::default();
        assert_eq!(s.fase, Fase::Rojo);
        s.activar(30.0); // fin del rojo
        assert_eq!(s.fase, Fase::Verde);
        assert!((s.sigma - 15.0).abs() < 1e-9);
        s.activar(45.0); // fin del verde
        assert_eq!(s.fase, Fase::Rojo);
        assert!((s.sigma - 30.0).abs() < 1e-9);
    }

    #[test]
    fn boton_temprano_acorta_a_20() {
        let mut s = SemaforoV2::default();
        s.boton = 1.0;
        s.activar(8.0); // δext a los 8 s (elapsed 8 < 20)
        assert_eq!(s.fase, Fase::Rojo);
        assert!((s.sigma - 12.0).abs() < 1e-9, "quedan 20-8=12 s de rojo");
        s.activar(20.0); // tick interno reprogramado
        assert_eq!(s.fase, Fase::Verde);
    }

    #[test]
    fn boton_tardio_cambia_inmediato() {
        let mut s = SemaforoV2::default();
        s.activar(25.0); // δext "vacía" (sin botón): solo avanza el tiempo
        s.boton = 1.0;
        s.activar(25.0); // δext con botón (elapsed 25 ≥ 20)
        assert!(s.sigma <= EPS, "σ=0 → cambio inmediato pendiente");
        s.activar(25.0); // re-activación en el mismo instante → δint
        assert_eq!(s.fase, Fase::Verde);
    }

    #[test]
    fn boton_en_verde_se_ignora() {
        let mut s = SemaforoV2::default();
        s.activar(30.0); // → verde
        s.boton = 1.0;
        s.activar(40.0); // δext durante el verde
        assert_eq!(s.fase, Fase::Verde);
        assert!((s.sigma - 5.0).abs() < 1e-9, "el verde sigue su curso (15-10=5)");
    }
}
