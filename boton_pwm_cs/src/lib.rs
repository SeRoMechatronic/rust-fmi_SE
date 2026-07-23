//! GENERADOR DE SEÑAL DE BOTÓN — FMU FMI 3.0 **Co-Simulation**.
//!
//! Produce una señal 0/`amplitud` pensada para alimentar la entrada `boton` del semáforo
//! de Scheduled Execution (`semaforoV2_se`). Con tres parámetros cubre los casos típicos:
//!
//! | Caso                         | Configuración                                     |
//! |------------------------------|---------------------------------------------------|
//! | **Pulso único** (botón real) | `periodo = 0` → un pulso en `t_inicio`, de `ancho_pulso` s |
//! | **PWM** (pulsos repetidos)   | `periodo > 0` → un pulso de `ancho_pulso` s cada `periodo` s |
//! | **Escalón**                  | `periodo = 0` y `ancho_pulso` muy grande → sube en `t_inicio` y se queda |
//!
//! La salida vale `amplitud` (por defecto 1.0) durante el pulso y 0.0 el resto del tiempo.
//! El semáforo interpreta `boton >= 0.5` como "pulsado".
//!
//! Valores por defecto: primer pulso a los 8 s (durante el primer rojo), de 2 s de ancho,
//! repitiéndose cada 40 s.

use fmi::fmi3::{Fmi3Error, Fmi3Res};
use fmi_export::{
    fmi3::{CSDoStepResult, Context, DefaultLoggingCategory, UserModel},
    FmuModel,
};

#[derive(FmuModel, Default, Debug)]
#[model(model_exchange = false, co_simulation = true, user_model = false)]
pub struct BotonPwm {
    /// Instante del primer pulso [s].
    #[variable(causality = Parameter, variability = Fixed, start = 8.0, initial = Exact)]
    t_inicio: f64,

    /// Periodo de repetición [s]. `0` (o negativo) = un solo pulso.
    #[variable(causality = Parameter, variability = Fixed, start = 40.0, initial = Exact)]
    periodo: f64,

    /// Duración del pulso en alto [s].
    #[variable(causality = Parameter, variability = Fixed, start = 2.0, initial = Exact)]
    ancho_pulso: f64,

    /// Valor de la señal durante el pulso (el semáforo pulsa con >= 0.5).
    #[variable(causality = Parameter, variability = Fixed, start = 1.0, initial = Exact)]
    amplitud: f64,

    /// Señal de botón — SALIDA (conectar a `boton` del semáforo).
    #[variable(causality = Output, variability = Discrete, start = 0.0)]
    salida: f64,
}

impl BotonPwm {
    /// Valor de la señal en el instante `t` (función pura: la FMU no tiene estado).
    fn senal_en(&self, t: f64) -> f64 {
        // Antes del primer pulso: apagado.
        if t < self.t_inicio || self.ancho_pulso <= 0.0 {
            return 0.0;
        }
        let transcurrido = t - self.t_inicio;

        // periodo <= 0 → un único pulso (o escalón, si el ancho es enorme).
        let fase = if self.periodo > 0.0 {
            transcurrido % self.periodo
        } else {
            transcurrido
        };

        if fase < self.ancho_pulso {
            self.amplitud
        } else {
            0.0
        }
    }
}

impl UserModel for BotonPwm {
    type LoggingCategory = DefaultLoggingCategory;

    /// La salida depende solo del tiempo: se evalúa en el instante actual.
    fn calculate_values(&mut self, context: &dyn Context<Self>) -> Result<Fmi3Res, Fmi3Error> {
        self.salida = self.senal_en(context.time());
        Ok(Fmi3Res::OK)
    }

    /// Co-Simulation: avanzamos al final del intervalo y evaluamos ahí la señal.
    fn do_step(
        &mut self,
        context: &mut dyn Context<Self>,
        current_communication_point: f64,
        communication_step_size: f64,
        _no_set_fmu_state_prior_to_current_point: bool,
    ) -> Result<CSDoStepResult, Fmi3Error> {
        let t_final = current_communication_point + communication_step_size;
        context.set_time(t_final);
        self.calculate_values(context)?;
        Ok(CSDoStepResult::completed(t_final))
    }
}

fmi_export::export_fmu!(BotonPwm);

// ── Tests de la señal (lógica pura, sin FFI) ──────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    /// Config por defecto: pulso de 2 s cada 40 s, empezando en t=8.
    fn gen() -> BotonPwm {
        let mut g = BotonPwm::default();
        g.t_inicio = 8.0;
        g.periodo = 40.0;
        g.ancho_pulso = 2.0;
        g.amplitud = 1.0;
        g
    }

    #[test]
    fn apagado_antes_del_inicio() {
        let g = gen();
        assert_eq!(g.senal_en(0.0), 0.0);
        assert_eq!(g.senal_en(7.9), 0.0);
    }

    #[test]
    fn pulso_en_la_ventana() {
        let g = gen();
        assert_eq!(g.senal_en(8.0), 1.0, "empieza el pulso");
        assert_eq!(g.senal_en(9.5), 1.0, "dentro del pulso");
        assert_eq!(g.senal_en(10.0), 0.0, "el pulso ya acabó (ancho 2 s)");
    }

    #[test]
    fn se_repite_cada_periodo() {
        let g = gen();
        assert_eq!(g.senal_en(48.0), 1.0, "segundo pulso a los 8+40");
        assert_eq!(g.senal_en(49.5), 1.0);
        assert_eq!(g.senal_en(50.5), 0.0);
        assert_eq!(g.senal_en(88.0), 1.0, "tercer pulso a los 8+80");
    }

    #[test]
    fn pulso_unico_si_periodo_cero() {
        let mut g = gen();
        g.periodo = 0.0;
        assert_eq!(g.senal_en(8.5), 1.0, "el único pulso");
        assert_eq!(g.senal_en(10.5), 0.0);
        assert_eq!(g.senal_en(48.5), 0.0, "no se repite");
    }

    #[test]
    fn escalon() {
        let mut g = gen();
        g.periodo = 0.0;
        g.ancho_pulso = 1.0e9; // "para siempre"
        assert_eq!(g.senal_en(7.9), 0.0);
        assert_eq!(g.senal_en(8.0), 1.0);
        assert_eq!(g.senal_en(1000.0), 1.0, "se queda arriba");
    }

    #[test]
    fn amplitud_configurable() {
        let mut g = gen();
        g.amplitud = 3.3;
        assert_eq!(g.senal_en(8.5), 3.3);
    }
}
