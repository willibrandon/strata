//! The simulation worker: owns the time-consuming work (running, replaying,
//! measuring activity) on its own thread, so the interface stays responsive.
//!
//! The session lives behind a mutex. The worker holds it for short slices
//! (about 20 ms) while running; the interface locks it briefly each frame to
//! copy what it displays.

use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use strata_debug::{inspect, Session, StopReason, TimelineEvent, TriggerKind};

#[derive(Debug, Clone)]
pub enum Command {
    Run,
    Pause,
    StepCycle,
    StepInstruction,
    StepLine,
    ReverseInstruction,
    Goto(u64),
    Record(TimelineEvent),
    AddTrigger(TriggerKind, String),
    RemoveTrigger(u32),
    ToggleTrigger(u32),
    Activity { start: u64, end: u64 },
    Waves { signals: Vec<strata_debug::Signal>, start: u64, end: u64 },
    SaveSession(std::path::PathBuf),
    Checkpoint,
    Shutdown,
}

/// State the interface shows about the worker itself.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub running: bool,
    pub busy: Option<String>,
    pub last_stop: Option<String>,
    /// Simulation speed while running, in cycles per second.
    pub speed: f64,
    /// Result of the last activity request.
    pub activity: Option<Arc<inspect::Activity>>,
    /// Result of the last waveform capture.
    pub waves: Option<Arc<inspect::Waves>>,
}

pub struct Worker {
    pub session: Arc<Mutex<Session>>,
    pub status: Arc<Mutex<Status>>,
    tx: Sender<Command>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(session: Session, repaint: Box<dyn Fn() + Send>) -> Worker {
        let session = Arc::new(Mutex::new(session));
        let status = Arc::new(Mutex::new(Status::default()));
        let (tx, rx) = channel();
        let (s, st) = (Arc::clone(&session), Arc::clone(&status));
        let handle = std::thread::Builder::new()
            .name("strata-sim".into())
            .spawn(move || run(s, st, rx, repaint))
            .expect("spawn simulation thread");
        Worker { session, status, tx, handle: Some(handle) }
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn describe(s: &Session, r: &StopReason) -> String {
    match r {
        StopReason::Trigger(id) => {
            let label = s.triggers.iter().find(|t| t.id == *id).map(|t| t.label.clone()).unwrap_or_default();
            format!("trigger {id} ({label}) at cycle {}", s.cycle())
        }
        StopReason::Halted => format!("machine halted with code {} at cycle {}", s.machine.last_out.halt_code, s.cycle()),
        StopReason::Budget => format!("paused at cycle {}", s.cycle()),
        StopReason::Stepped => format!("cycle {}", s.cycle()),
    }
}

/// Runs until the source line changes (or a trigger/halt), stepping by
/// instructions.
fn step_line(s: &mut Session) -> StopReason {
    let loc = |s: &Session, pc: u32| {
        let prog = inspect::slot_program(s, pc);
        s.debug.image_at(pc, prog.as_deref()).and_then(|i| i.source_line(pc))
    };
    let start = s.retired.last().and_then(|r| loc(s, r.pc));
    for _ in 0..100_000 {
        let r = s.step_instruction(100_000);
        if r != StopReason::Stepped || s.machine.halted() {
            return r;
        }
        let here = s.retired.last().and_then(|r| loc(s, r.pc));
        if here.is_some() && here != start {
            return StopReason::Stepped;
        }
    }
    StopReason::Budget
}

fn run(session: Arc<Mutex<Session>>, status: Arc<Mutex<Status>>, rx: Receiver<Command>, repaint: Box<dyn Fn() + Send>) {
    let set_busy = |b: Option<&str>| status.lock().unwrap().busy = b.map(str::to_string);
    let mut running = false;
    // Cycles per slice, adapted to take about 20 ms.
    let mut slice: u64 = 1000;
    loop {
        let cmd = if running {
            match rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        } else {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(c) => Some(c),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => return,
            }
        };
        if let Some(cmd) = cmd {
            let mut s = session.lock().unwrap();
            let stop = match cmd {
                Command::Run => {
                    running = true;
                    None
                }
                Command::Pause => {
                    running = false;
                    Some(format!("paused at cycle {}", s.cycle()))
                }
                Command::StepCycle => {
                    let r = s.step_cycle();
                    Some(describe(&s, &r))
                }
                Command::StepInstruction => {
                    let r = s.step_instruction(1_000_000);
                    Some(describe(&s, &r))
                }
                Command::StepLine => {
                    set_busy(Some("stepping to the next source line"));
                    let r = step_line(&mut s);
                    Some(describe(&s, &r))
                }
                Command::ReverseInstruction => {
                    set_busy(Some("replaying"));
                    s.reverse_step_instruction();
                    Some(format!("back to cycle {}", s.cycle()))
                }
                Command::Goto(c) => {
                    set_busy(Some("replaying"));
                    s.goto(c);
                    Some(format!("at cycle {}", s.cycle()))
                }
                Command::Record(e) => {
                    s.record(e);
                    None
                }
                Command::AddTrigger(k, label) => {
                    s.add_trigger(k, label);
                    None
                }
                Command::RemoveTrigger(id) => {
                    s.remove_trigger(id);
                    None
                }
                Command::ToggleTrigger(id) => {
                    if let Some(t) = s.triggers.iter_mut().find(|t| t.id == id) {
                        t.enabled = !t.enabled;
                    }
                    None
                }
                Command::Activity { start, end } => {
                    set_busy(Some("measuring activity"));
                    let a = inspect::activity(&mut s, start, end);
                    status.lock().unwrap().activity = Some(Arc::new(a));
                    None
                }
                Command::Waves { signals, start, end } => {
                    set_busy(Some("capturing waveforms"));
                    let w = inspect::capture(&mut s, &signals, start, end);
                    status.lock().unwrap().waves = Some(Arc::new(w));
                    None
                }
                Command::SaveSession(path) => {
                    let f = s.to_file();
                    let r = serde_json::to_string_pretty(&f)
                        .map_err(|e| e.to_string())
                        .and_then(|j| std::fs::write(&path, j).map_err(|e| e.to_string()));
                    Some(match r {
                        Ok(()) => format!("session saved to {}", path.display()),
                        Err(e) => format!("could not save session: {e}"),
                    })
                }
                Command::Checkpoint => {
                    s.checkpoint_now();
                    None
                }
                Command::Shutdown => return,
            };
            drop(s);
            set_busy(None);
            let mut st = status.lock().unwrap();
            st.running = running;
            if let Some(m) = stop {
                st.last_stop = Some(m);
            }
            drop(st);
            repaint();
        }
        if running {
            let t0 = Instant::now();
            let mut s = session.lock().unwrap();
            let c0 = s.cycle();
            let r = s.run(slice, &mut || false);
            let done = s.cycle() - c0;
            let msg = (r != StopReason::Budget).then(|| describe(&s, &r));
            drop(s);
            let dt = t0.elapsed().as_secs_f64().max(1e-6);
            slice = ((done as f64 / dt) * 0.02).clamp(100.0, 2_000_000.0) as u64;
            let mut st = status.lock().unwrap();
            st.speed = done as f64 / dt;
            if let Some(m) = msg {
                running = false;
                st.last_stop = Some(m);
            }
            st.running = running;
            drop(st);
            repaint();
        }
    }
}
