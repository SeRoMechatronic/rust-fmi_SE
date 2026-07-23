#![allow(unexpected_cfgs)]
//! Modelo TÉRMICO del motor: se calienta con la carga (∝ mando²) y se enfría
//! hacia el ambiente.
//!
//!   C * dT/dt = k_loss * carga²  -  (T - T_amb) / R_th
//!
//! Corre en el **ESP32 WROOM**. Recibe la carga `carga` (el mando `u` del control
//! de velocidad) y entrega la temperatura del motor `temp`, que el supervisor
//! vigila para la parada de seguridad. FMI 3.0 (ME + CS).

use fmi::fmi3::{Fmi3Error, Fmi3Res};
use fmi_export::{
    fmi3::{CSDoStepResult, Context, DefaultLoggingCategory, UserModel},
    FmuModel,
};

#[derive(FmuModel, Default, Debug)]
#[model(model_exchange = true, co_simulation = true, user_model = false)]
struct MotorTermico {
    /// Carga del motor (típicamente el mando u del control de velocidad) — entrada
    #[variable(causality = Input, variability = Continuous, start = 0.0)]
    carga: f64,

    /// Temperatura del motor [°C] (salida, estado continuo)
    #[variable(causality = Output, variability = Continuous, start = 25.0, initial = Exact)]
    temp: f64,

    /// Derivada de la temperatura (define temp como estado)
    #[variable(causality = Local, variability = Continuous, derivative = temp, initial = Calculated)]
    der_temp: f64,

    /// Temperatura ambiente [°C]
    #[variable(causality = Parameter, variability = Fixed, start = 25.0, initial = Exact)]
    t_amb: f64,

    /// Calor generado por unidad de carga² [°C/s por unidad²]
    #[variable(causality = Parameter, variability = Fixed, start = 90.0, initial = Exact)]
    k_loss: f64,

    /// Resistencia térmica (mayor R → se calienta más) [°C·s / °C]
    #[variable(causality = Parameter, variability = Fixed, start = 1.0, initial = Exact)]
    r_th: f64,

    /// Capacidad térmica (mayor C → más lento) [s]
    #[variable(causality = Parameter, variability = Fixed, start = 10.0, initial = Exact)]
    c_th: f64,
}

impl UserModel for MotorTermico {
    type LoggingCategory = DefaultLoggingCategory;

    fn calculate_values(&mut self, _context: &dyn Context<Self>) -> Result<Fmi3Res, Fmi3Error> {
        let cooling = if self.r_th != 0.0 { (self.temp - self.t_amb) / self.r_th } else { 0.0 };
        let heating = self.k_loss * self.carga * self.carga;
        self.der_temp = if self.c_th != 0.0 { (heating - cooling) / self.c_th } else { 0.0 };
        Ok(Fmi3Res::OK)
    }

    fn do_step(
        &mut self,
        context: &mut dyn Context<Self>,
        current_communication_point: f64,
        communication_step_size: f64,
        _no_set_fmu_state_prior_to_current_point: bool,
    ) -> Result<CSDoStepResult, Fmi3Error> {
        context.set_time(current_communication_point);
        self.calculate_values(context)?;
        self.temp += self.der_temp * communication_step_size; // Euler
        let last = current_communication_point + communication_step_size;
        context.set_time(last);
        Ok(CSDoStepResult::completed(last))
    }
}

fmi_export::export_fmu!(MotorTermico);
