//! GENERADOR DE PULSACIONES DE BOTÓN — FMU FMI 3.0 **Co-Simulation**.
//!
//! Produce una señal 0/`amplitud` con hasta **4 pulsaciones en instantes que tú eliges**,
//! pensada para alimentar la entrada `boton` del semáforo de Scheduled Execution.
//!
//! Cada pulsación `t_pulsoN` dispara un pulso de `ancho_pulso` segundos. Un valor
//! **negativo** desactiva esa pulsación, así que puedes usar 1, 2, 3 o 4.
//!
//! | Quiero…                        | Configuración                                  |
//! |--------------------------------|------------------------------------------------|
//! | 4 pulsaciones repartidas       | `t_pulso1..4` = los instantes que quieras       |
//! | solo 2 pulsaciones             | `t_pulso3 = -1`, `t_pulso4 = -1`                |
//! | un escalón (sube y se queda)   | `t_pulso1 = T`, `ancho_pulso` enorme (p.ej. 1e9)|
//!
//! Valores por defecto: pulsaciones a los **8, 45, 95 y 118 s**, de 2 s cada una.
//! Están elegidas para caer dentro de fases ROJAS del semáforo y así demostrar los
//! tres comportamientos: acortar el rojo, cambio inmediato (botón tardío) y otro
//! acortamiento.
//!
//! El semáforo interpreta `boton >= 0.5` como "pulsado".
//!
//! Value References: `t_pulso1`=1, `t_pulso2`=2, `t_pulso3`=3, `t_pulso4`=4,
//! `ancho_pulso`=5, `amplitud`=6, **`salida`=7**.

use fmi::fmi3::{Fmi3Error, Fmi3Res};
use fmi_export::{
    fmi3::{CSDoStepResult, Context, DefaultLoggingCategory, UserModel},
    FmuModel,
};

#[derive(FmuModel, Default, Debug)]
#[model(model_exchange = false, co_simulation = true, user_model = false)]
pub struct BotonPwm {
    /// Instante de la 1ª pulsación [s]. Negativo = desactivada.
    #[variable(causality = Parameter, variability = Fixed, start = 8.0, initial = Exact)]
    t_pulso1: f64,

    /// Instante de la 2ª pulsación [s]. Negativo = desactivada.
    #[variable(causality = Parameter, variability = Fixed, start = 45.0, initial = Exact)]
    t_pulso2: f64,

    /// Instante de la 3ª pulsación [s]. Negativo = desactivada.
    #[variable(causality = Parameter, variability = Fixed, start = 95.0, initial = Exact)]
    t_pulso3: f64,

    /// Instante de la 4ª pulsación [s]. Negativo = desactivada.
    #[variable(causality = Parameter, variability = Fixed, start = 118.0, initial = Exact)]
    t_pulso4: f64,

    /// Duración de cada pulsación [s].
    #[variable(causality = Parameter, variability = Fixed, start = 2.0, initial = Exact)]
    ancho_pulso: f64,

    /// Valor de la señal durante la pulsación (el semáforo pulsa con >= 0.5).
    #[variable(causality = Parameter, variability = Fixed, start = 1.0, initial = Exact)]
    amplitud: f64,

    /// Señal de botón — SALIDA (conectar a `boton` del semáforo).
    #[variable(causality = Output, variability = Discrete, start = 0.0)]
    salida: f64,
}

impl BotonPwm {
    /// Valor de la señal en el instante `t` (función pura: la FMU no tiene estado).
    fn senal_en(&self, t: f64) -> f64 {
        if self.ancho_pulso <= 0.0 {
            return 0.0;
        }
        let pulsaciones = [self.t_pulso1, self.t_pulso2, self.t_pulso3, self.t_pulso4];
        for tp in pulsaciones {
            // Negativo = pulsación desactivada.
            if tp >= 0.0 && t >= tp && t < tp + self.ancho_pulso {
                return self.amplitud;
            }
        }
        0.0
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

    /// Config por defecto: pulsaciones a 8, 45, 95 y 118 s, de 2 s.
    fn gen() -> BotonPwm {
        let mut g = BotonPwm::default();
        g.t_pulso1 = 8.0;
        g.t_pulso2 = 45.0;
        g.t_pulso3 = 95.0;
        g.t_pulso4 = 118.0;
        g.ancho_pulso = 2.0;
        g.amplitud = 1.0;
        g
    }

    #[test]
    fn apagado_fuera_de_las_pulsaciones() {
        let g = gen();
        assert_eq!(g.senal_en(0.0), 0.0);
        assert_eq!(g.senal_en(7.9), 0.0);
        assert_eq!(g.senal_en(30.0), 0.0);
        assert_eq!(g.senal_en(199.0), 0.0);
    }

    #[test]
    fn cada_pulsacion_dura_su_ancho() {
        let g = gen();
        for tp in [8.0, 45.0, 95.0, 118.0] {
            assert_eq!(g.senal_en(tp), 1.0, "inicio del pulso en {tp}");
            assert_eq!(g.senal_en(tp + 1.9), 1.0, "dentro del pulso en {tp}");
            assert_eq!(g.senal_en(tp + 2.0), 0.0, "el pulso acabó en {tp}");
        }
    }

    #[test]
    fn negativo_desactiva_la_pulsacion() {
        let mut g = gen();
        g.t_pulso3 = -1.0;
        g.t_pulso4 = -1.0;
        assert_eq!(g.senal_en(8.5), 1.0, "la 1ª sigue activa");
        assert_eq!(g.senal_en(45.5), 1.0, "la 2ª sigue activa");
        assert_eq!(g.senal_en(95.5), 0.0, "la 3ª está desactivada");
        assert_eq!(g.senal_en(118.5), 0.0, "la 4ª está desactivada");
    }

    #[test]
    fn escalon_con_ancho_enorme() {
        let mut g = gen();
        g.t_pulso2 = -1.0;
        g.t_pulso3 = -1.0;
        g.t_pulso4 = -1.0;
        g.ancho_pulso = 1.0e9;
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
