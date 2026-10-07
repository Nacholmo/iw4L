use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use crate::media::{PcmError, RenderMedia};
use crate::render_core::InstanceState;

#[derive(Clone)]
pub struct ExternalClip {
    pub(crate) media: RenderMedia,
}

impl ExternalClip {
    pub fn from_pcm(samples: Arc<[f32]>, channels: u16, rate: u32) -> Result<Self, PcmError> {
        RenderMedia::from_pcm(samples, channels, rate).map(|media| Self { media })
    }
}

#[derive(Clone, Default)]
pub struct ExternalSound(pub(crate) Arc<OnceLock<Arc<InstanceState>>>);

impl ExternalSound {
    pub fn stop(&self) {
        if let Some(instance) = self.0.get() {
            instance.stopped.store(true, Ordering::Release);
        }
    }
}

pub(crate) struct ExternalStart {
    pub media: RenderMedia,
    pub gain: f32,
    pub handle: ExternalSound,
}
