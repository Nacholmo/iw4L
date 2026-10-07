use std::collections::HashMap;
use std::sync::Arc;

use audio::{AudioRuntime, ExternalClip, ExternalSound};
use dis_data::sounds::{Cue, Gait, Sounds, Surface};
use dis_motion::{BlinkEvent, MantleKind, Motion, MotionState, StepEvents};
use wwise::PlayNode;

const MIX_GAIN: f32 = 1.6;

pub struct SoundBank {
    cues: HashMap<Cue, PlayNode>,
    clips: HashMap<u32, ExternalClip>,
}

pub fn decode(sounds: Sounds, warnings: &mut Vec<String>) -> SoundBank {
    let mut clips = HashMap::new();
    for (id, ogg) in sounds.ogg {
        match ogg_to_clip(&ogg) {
            Ok(clip) => {
                clips.insert(id, clip);
            }
            Err(e) => warnings.push(format!("sound {id:#x}: {e}")),
        }
    }
    SoundBank {
        cues: sounds.cues,
        clips,
    }
}

fn ogg_to_clip(ogg: &[u8]) -> Result<ExternalClip, String> {
    let mut reader = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(ogg))
        .map_err(|e| e.to_string())?;
    let channels = u16::from(reader.ident_hdr.audio_channels);
    let rate = reader.ident_hdr.audio_sample_rate;
    let mut samples = Vec::new();
    while let Some(packet) = reader.read_dec_packet_itl().map_err(|e| e.to_string())? {
        samples.extend(packet.into_iter().map(|s| f32::from(s) / 32768.0));
    }
    ExternalClip::from_pcm(Arc::from(samples), channels, rate).map_err(|e| format!("{e:?}"))
}

pub struct Sfx {
    rng: u64,
    step_accum: f32,
    climb_accum: f32,
    sprint_time: f32,
    breath_timer: f32,
    swim_timer: f32,
    prev_state: MotionState,
    prev_crouched: bool,
    warmup: Vec<ExternalSound>,
    fall_wind: Vec<ExternalSound>,
    pub steps: Vec<(Surface, dis_motion::Vec3)>,
}

impl Default for Sfx {
    fn default() -> Self {
        Self {
            rng: 0x9E37_79B9_7F4A_7C15,
            step_accum: 0.0,
            climb_accum: 0.0,
            sprint_time: 0.0,
            breath_timer: 0.0,
            swim_timer: 0.0,
            prev_state: MotionState::Falling,
            prev_crouched: false,
            warmup: Vec::new(),
            fall_wind: Vec::new(),
            steps: Vec::new(),
        }
    }
}

pub struct Frame<'a> {
    pub bank: &'a SoundBank,
    pub runtime: Option<&'a AudioRuntime>,
    pub motion: &'a Motion,
    pub events: &'a StepEvents,
    pub moved: dis_motion::Vec3,
    pub surface: Surface,
    pub dt: f32,
}

impl Sfx {
    fn rand(&mut self, n: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % n.max(1) as u64) as usize
    }

    fn play(&mut self, f: &Frame, cue: Cue, volume: f32) -> Vec<ExternalSound> {
        let (Some(runtime), Some(node)) = (f.runtime, f.bank.cues.get(&cue)) else {
            return Vec::new();
        };
        let mut ids = node.choose(&mut |n| self.rand(n));
        ids.dedup();
        ids.into_iter()
            .filter_map(|id| f.bank.clips.get(&id))
            .map(|clip| runtime.play_external(clip, volume * MIX_GAIN))
            .collect()
    }

    pub fn silence(&mut self) {
        for s in self.warmup.drain(..).chain(self.fall_wind.drain(..)) {
            s.stop();
        }
    }

    pub fn update(&mut self, f: &Frame) {
        let m = f.motion;
        let ev = f.events;
        let feet = m.feet();
        let surface = f.surface;
        let gait = if m.crouched {
            Gait::Sneak
        } else if m.sprinting {
            Gait::Sprint
        } else {
            Gait::Run
        };

        let moved = f.moved.truncate().length();
        if m.state == MotionState::Walking && moved < 100.0 {
            self.step_accum += moved;
            let stride = match gait {
                Gait::Sneak => 100.0,
                Gait::Run => 150.0,
                Gait::Sprint => 210.0,
            };
            if self.step_accum >= stride {
                self.step_accum = 0.0;
                let vol = if gait == Gait::Sneak { 0.35 } else { 0.6 };
                self.play(f, Cue::Footstep(surface, gait), vol);
                self.steps.push((surface, feet));
            }
        } else if m.state != MotionState::Walking {
            self.step_accum = 75.0;
        }
        if m.state == MotionState::Ladder {
            self.climb_accum += f.moved.z.abs();
            if self.climb_accum >= 45.0 {
                self.climb_accum = 0.0;
                self.play(f, Cue::Footstep(Surface::Metal, Gait::Sneak), 0.5);
            }
        }
        if m.state == MotionState::Swimming && moved > 0.5 {
            self.swim_timer -= f.dt;
            if self.swim_timer <= 0.0 {
                self.swim_timer = 1.1;
                self.play(f, Cue::Swim, 0.6);
            }
        }

        if ev.jumped {
            self.play(f, Cue::Jump, 0.5);
            self.play(f, Cue::Footstep(surface, Gait::Run), 0.5);
        }
        if let Some(impact) = ev.landed {
            for s in std::mem::take(&mut self.fall_wind) {
                s.stop();
            }
            if impact > 1100.0 {
                self.play(f, Cue::LandHigh(surface), 0.8);
            } else if impact > 250.0 {
                self.play(f, Cue::LandSmall(surface), 0.6);
            }
        }
        if m.state == MotionState::Falling && m.vel.z < -900.0 && self.fall_wind.is_empty() {
            self.fall_wind = self.play(f, Cue::FallWind, 0.5);
        }
        if let Some(kind) = ev.mantled {
            let cue = match kind {
                MantleKind::Low => Cue::MantleLow,
                MantleKind::Medium => Cue::MantleMedium,
                MantleKind::High => Cue::MantleHigh,
            };
            self.play(f, cue, 0.7);
        }
        if self.prev_state == MotionState::Mantling && m.state == MotionState::Walking {
            self.play(f, Cue::MantleImpact, 0.6);
        }
        if ev.slid {
            self.play(f, Cue::Slide, 0.7);
        }
        if m.state == MotionState::Swimming && self.prev_state != MotionState::Swimming {
            self.play(f, Cue::WaterEnter, 0.7);
        }
        let crouch_by_player =
            m.state == MotionState::Walking && self.prev_state == MotionState::Walking;
        if m.crouched != self.prev_crouched && crouch_by_player {
            self.play(f, if m.crouched { Cue::Crouch } else { Cue::Stand }, 0.45);
        }

        if m.sprinting && m.state == MotionState::Walking {
            self.sprint_time += f.dt;
            self.breath_timer -= f.dt;
            if self.sprint_time > 3.0 && self.breath_timer <= 0.0 {
                self.breath_timer = 4.0;
                self.play(f, Cue::SprintBreath, 0.5);
            }
        } else {
            self.sprint_time = 0.0;
        }

        for b in &ev.blink {
            match b {
                BlinkEvent::StartedTargeting => {
                    self.warmup = self.play(f, Cue::BlinkWarmup, 0.8);
                }
                BlinkEvent::Released | BlinkEvent::Fizzled => {
                    for s in std::mem::take(&mut self.warmup) {
                        s.stop();
                    }
                    let cue = if *b == BlinkEvent::Released {
                        Cue::BlinkRelease
                    } else {
                        Cue::BlinkFizzle
                    };
                    self.play(f, cue, 0.9);
                }
                _ => {}
            }
        }
        self.prev_state = m.state;
        self.prev_crouched = m.crouched;
    }
}
