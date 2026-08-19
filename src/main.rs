use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const PREP_SECONDS: u64 = 3;
const EXPECTED_MAX_SECONDS: u64 = 30;

fn record(expected_max_seconds: u64) -> (Vec<f32>, u32) {
    let host = cpal::default_host();
    let device = host.default_input_device().expect("no input device found");
    let config = device.default_input_config().expect("no default config");

    let sample_rate = config.sample_rate();
    let channels = config.channels() as usize;
    println!("recording from {device} at {sample_rate} Hz, {channels} channel(s)");

    let expected_samples = sample_rate as usize * channels * expected_max_seconds as usize;
    let samples = Arc::new(Mutex::new(Vec::<f32>::with_capacity(expected_samples)));
    let samples_for_callback = samples.clone();

    let err_fn = move |err| {
        eprintln!("stream error: {err}");
    };

    let stream = device
        .build_input_stream(
            config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                samples_for_callback.lock().unwrap().extend_from_slice(data);
            },
            err_fn,
            None,
        )
        .expect("failed to build input stream");

    println!("get ready to hum...");
    for n in (1..=PREP_SECONDS).rev() {
        println!("{n}...");
        std::thread::sleep(Duration::from_secs(1));
    }

    stream.play().expect("failed to start the stream");
    println!("recording now! hum something, then press enter when you are done");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).expect("failed to read input");

    drop(stream);

    let interleaved = samples.lock().unwrap();
    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();

    (mono, sample_rate)
}

fn main() {
    let (clip, sample_rate) = record(EXPECTED_MAX_SECONDS);
    println!(
        "mono clip: {} samples at {} Hz ({:.2}s)",
        clip.len(),
        sample_rate,
        clip.len() as f32 / sample_rate as f32
    );
}
