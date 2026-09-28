//! RTL mode: the SystemVerilog of `strata_soc`, compiled by Verilator, as a
//! [`strata_sim::Engine`]. Used for long runs and as the reference in
//! RTL-versus-gate lockstep comparisons. Flip-flops start at zero, like
//! `strata_gate::PowerOn::Zero`.

use std::ffi::{c_char, c_void, CStr};

use strata_sim::ports::{output_setters, SocIn, SocOut, SOC_INPUTS, SOC_OUTPUTS};
use strata_sim::Engine;

extern "C" {
    fn strata_rtl_available() -> i32;
    fn strata_rtl_input_name(i: i32) -> *const c_char;
    fn strata_rtl_output_name(i: i32) -> *const c_char;
    fn strata_rtl_new() -> *mut c_void;
    fn strata_rtl_free(p: *mut c_void);
    fn strata_rtl_set_inputs(p: *mut c_void, v: *const u64);
    fn strata_rtl_eval(p: *mut c_void);
    fn strata_rtl_tick(p: *mut c_void);
    fn strata_rtl_outputs(p: *mut c_void, o: *mut u64);
}

pub struct RtlSim {
    p: *mut c_void,
    setters: Vec<fn(&mut SocOut, u64)>,
    pub cycles: u64,
}

// The model is only ever used from one thread at a time.
unsafe impl Send for RtlSim {}

fn port_names(f: unsafe extern "C" fn(i32) -> *const c_char) -> Vec<String> {
    let mut v = Vec::new();
    for i in 0.. {
        // SAFETY: returns a static NUL-terminated string or null past the end.
        let p = unsafe { f(i) };
        if p.is_null() {
            break;
        }
        v.push(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
    }
    v
}

impl RtlSim {
    pub fn available() -> bool {
        unsafe { strata_rtl_available() != 0 }
    }

    pub fn new() -> Result<RtlSim, String> {
        if !Self::available() {
            return Err("RTL engine unavailable: strata-rtl was built without Verilator".into());
        }
        let ins = port_names(strata_rtl_input_name);
        let outs = port_names(strata_rtl_output_name);
        let want_in: Vec<&str> = SOC_INPUTS.iter().map(|p| p.0).collect();
        let want_out: Vec<&str> = SOC_OUTPUTS.iter().map(|p| p.0).collect();
        if ins != want_in || outs != want_out {
            return Err("RTL shim port order does not match strata-sim".into());
        }
        let p = unsafe { strata_rtl_new() };
        Ok(RtlSim { p, setters: output_setters(), cycles: 0 })
    }
}

impl Drop for RtlSim {
    fn drop(&mut self) {
        unsafe { strata_rtl_free(self.p) }
    }
}

impl Engine for RtlSim {
    fn describe(&self) -> String {
        "rtl: Verilator model of strata_soc".into()
    }

    fn set_inputs(&mut self, inputs: &SocIn) {
        let v = inputs.to_array();
        unsafe { strata_rtl_set_inputs(self.p, v.as_ptr()) }
    }

    fn eval(&mut self) {
        unsafe { strata_rtl_eval(self.p) }
    }

    fn outputs(&self) -> SocOut {
        let mut raw = [0u64; 46];
        unsafe { strata_rtl_outputs(self.p, raw.as_mut_ptr()) };
        let mut o = SocOut::default();
        for (i, v) in raw.iter().enumerate() {
            (self.setters[i])(&mut o, *v);
        }
        o
    }

    fn tick(&mut self) {
        unsafe { strata_rtl_tick(self.p) }
        self.cycles += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_match() {
        if !RtlSim::available() {
            eprintln!("skipping: no Verilator");
            return;
        }
        assert_eq!(SOC_OUTPUTS.len(), 46);
        RtlSim::new().unwrap();
    }
}
