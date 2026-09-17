use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use rtrb::{Consumer, Producer, RingBuffer};

#[derive(Debug)]
pub struct AudioBlock {
    samples: Box<[f32]>,
    pub valid_samples: usize,
    pub sequence: u64,
    pub start_frame: u64,
    pub timestamp_nanos: Option<u64>,
}

impl AudioBlock {
    pub fn samples(&self) -> &[f32] {
        &self.samples[..self.valid_samples]
    }
}

#[derive(Debug, Default)]
pub struct CaptureCounters {
    pub unavailable_blocks: AtomicU64,
    pub saturated_blocks: AtomicU64,
    pub dropped_frames: AtomicU64,
}

pub struct CaptureCallback {
    free: Consumer<AudioBlock>,
    filled: Producer<AudioBlock>,
    current: Option<AudioBlock>,
    channels: usize,
    sequence: u64,
    next_frame: u64,
    counters: Arc<CaptureCounters>,
}

pub struct CaptureWorker {
    free: Producer<AudioBlock>,
    filled: Consumer<AudioBlock>,
    pub counters: Arc<CaptureCounters>,
}

impl CaptureCallback {
    pub fn new(pool_blocks: usize, block_frames: usize, channels: usize) -> (Self, CaptureWorker) {
        assert!(pool_blocks > 1 && block_frames > 0 && channels > 0);
        let (mut free_tx, free_rx) = RingBuffer::new(pool_blocks);
        let (filled_tx, filled_rx) = RingBuffer::new(pool_blocks);
        for _ in 0..pool_blocks {
            let block = AudioBlock {
                samples: vec![0.0; block_frames * channels].into_boxed_slice(),
                valid_samples: 0,
                sequence: 0,
                start_frame: 0,
                timestamp_nanos: None,
            };
            assert!(free_tx.push(block).is_ok());
        }
        let counters = Arc::new(CaptureCounters::default());
        (
            Self {
                free: free_rx,
                filled: filled_tx,
                current: None,
                channels,
                sequence: 0,
                next_frame: 0,
                counters: Arc::clone(&counters),
            },
            CaptureWorker {
                free: free_tx,
                filled: filled_rx,
                counters,
            },
        )
    }

    /// Callback-safe: bounded copies and wait-free ring operations only.
    pub fn capture(&mut self, mut input: &[f32], timestamp_nanos: Option<u64>) {
        while !input.is_empty() {
            if self.current.is_none() {
                let Ok(mut block) = self.free.pop() else {
                    let dropped_frames = input.len() / self.channels;
                    self.counters
                        .unavailable_blocks
                        .fetch_add(1, Ordering::Relaxed);
                    self.counters
                        .dropped_frames
                        .fetch_add(dropped_frames as u64, Ordering::Relaxed);
                    self.next_frame += dropped_frames as u64;
                    return;
                };
                block.valid_samples = 0;
                block.sequence = self.sequence;
                block.start_frame = self.next_frame;
                block.timestamp_nanos = timestamp_nanos;
                self.current = Some(block);
            }
            let Some(block) = self.current.as_mut() else {
                return;
            };
            let count = input.len().min(block.samples.len() - block.valid_samples);
            block.samples[block.valid_samples..block.valid_samples + count]
                .copy_from_slice(&input[..count]);
            block.valid_samples += count;
            self.next_frame += (count / self.channels) as u64;
            input = &input[count..];
            if block.valid_samples == block.samples.len() {
                let Some(block) = self.current.take() else {
                    return;
                };
                match self.filled.push(block) {
                    Ok(()) => self.sequence += 1,
                    Err(rtrb::PushError::Full(block)) => {
                        self.current = Some(block);
                        self.counters
                            .saturated_blocks
                            .fetch_add(1, Ordering::Relaxed);
                        let dropped_frames = input.len() / self.channels;
                        self.counters
                            .dropped_frames
                            .fetch_add(dropped_frames as u64, Ordering::Relaxed);
                        self.next_frame += dropped_frames as u64;
                        return;
                    }
                }
            }
        }
    }

    pub fn flush_partial(&mut self) {
        if let Some(block) = self.current.take() {
            if block.valid_samples > 0 {
                if let Err(rtrb::PushError::Full(block)) = self.filled.push(block) {
                    self.current = Some(block);
                    self.counters
                        .saturated_blocks
                        .fetch_add(1, Ordering::Relaxed);
                } else {
                    self.sequence += 1;
                }
            } else {
                self.current = Some(block);
            }
        }
    }
}

impl CaptureWorker {
    pub fn pop(&mut self) -> Option<AudioBlock> {
        self.filled.pop().ok()
    }

    pub fn recycle(&mut self, mut block: AudioBlock) -> Result<(), AudioBlock> {
        block.valid_samples = 0;
        self.free
            .push(block)
            .map_err(|rtrb::PushError::Full(block)| block)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Meter {
    pub peak: f32,
    pub rms: f32,
    pub clipped_samples: u64,
    pub signal_present: bool,
}

impl Meter {
    pub fn calculate(samples: impl Iterator<Item = f32>) -> Self {
        let mut meter = Self::default();
        let mut sum_squares = 0.0_f64;
        let mut count = 0_u64;
        for sample in samples {
            let magnitude = sample.abs();
            meter.peak = meter.peak.max(magnitude);
            meter.clipped_samples += u64::from(magnitude >= 1.0);
            sum_squares += f64::from(sample) * f64::from(sample);
            count += 1;
        }
        meter.rms = if count == 0 {
            0.0
        } else {
            (sum_squares / count as f64).sqrt() as f32
        };
        meter.signal_present = meter.peak >= 0.000_1;
        meter
    }
}

#[derive(Debug, Clone)]
pub struct SimulatedAudio {
    pub channels: usize,
    pub sample_rate: u32,
    next_frame: u64,
}

impl SimulatedAudio {
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        Self {
            channels,
            sample_rate,
            next_frame: 0,
        }
    }

    pub fn generate(&mut self, frames: usize, output: &mut Vec<f32>) {
        output.clear();
        output.reserve(frames * self.channels - output.capacity().min(frames * self.channels));
        for frame in 0..frames as u64 {
            for channel in 0..self.channels {
                output.push(channel as f32 * 1_000_000.0 + (self.next_frame + frame) as f32);
            }
        }
        self.next_frame += frames as u64;
    }
}

pub fn convert_i16(input: &[i16], output: &mut [f32]) {
    for (source, target) in input.iter().zip(output) {
        *target = f32::from(*source) / 32_768.0;
    }
}

pub fn convert_u16(input: &[u16], output: &mut [f32]) {
    for (source, target) in input.iter().zip(output) {
        *target = (f32::from(*source) - 32_768.0) / 32_768.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_callbacks_are_reassembled_without_duplication() {
        let (mut callback, mut worker) = CaptureCallback::new(8, 4, 2);
        let input: Vec<_> = (0..20).map(|value| value as f32).collect();
        for chunk in input.chunks(3) {
            callback.capture(chunk, Some(1));
            while let Some(block) = worker.pop() {
                assert_eq!(block.samples().len(), 8);
                worker.recycle(block).unwrap();
            }
        }
        callback.flush_partial();
        let partial = worker.pop().unwrap();
        assert_eq!(partial.samples(), &[16.0, 17.0, 18.0, 19.0]);
        assert_eq!(callback.counters.dropped_frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn callback_drops_instead_of_waiting_when_pool_is_exhausted() {
        let (mut callback, _worker) = CaptureCallback::new(2, 1, 1);
        callback.capture(&[1.0, 2.0, 3.0], None);
        assert!(callback.counters.dropped_frames.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn simulated_channels_have_detectable_identity() {
        let mut simulation = SimulatedAudio::new(2, 48_000);
        let mut output = Vec::with_capacity(8);
        simulation.generate(4, &mut output);
        assert_eq!(
            output,
            [
                0.0,
                1_000_000.0,
                1.0,
                1_000_001.0,
                2.0,
                1_000_002.0,
                3.0,
                1_000_003.0
            ]
        );
        let capacity = output.capacity();
        simulation.generate(4, &mut output);
        assert_eq!(output.capacity(), capacity);
    }

    #[test]
    fn meters_and_integer_conversion_are_bounded() {
        let meter = Meter::calculate([0.0, -0.5, 1.0].into_iter());
        assert_eq!(meter.peak, 1.0);
        assert_eq!(meter.clipped_samples, 1);
        let mut output = [0.0; 2];
        convert_i16(&[i16::MIN, i16::MAX], &mut output);
        assert_eq!(output[0], -1.0);
    }
}
