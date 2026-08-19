use std::sync::{Arc,Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait,HostTrait,StreamTrait};

//PrepTime
const PREP_SECONDS: u64=3;
const RECORD_SECONDS: u64=5;

fn main(){
    //Device Config
    let host=cpal::default_host();
    let device=host.default_input_device().expect("no input device found");
    let config=device.default_input_config().expect("no default config");

    let sample_rate=config.sample_rate();
    let channels=config.channels() as usize;
    println!("recording from {device} at {sample_rate} Hz, {channels} channel(s)");

    //Shared Buffer
    let expected_samples=sample_rate as usize*channels*RECORD_SECONDS as usize;
    let samples=Arc::new(Mutex::new(Vec::<f32>::with_capacity(expected_samples)));
    let samples_for_callback=samples.clone();

    //Error Callback
    let err_fn=move |err|{
        eprintln!("stream error: {err}");
    };

    //Data Callback and stream building
    let stream=device.build_input_stream(
        config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo|{
            samples_for_callback.lock().unwrap().extend_from_slice(data);
        },
        err_fn,
        None,
    )
    .expect("failed to build input stream");

    //PrepTime
    println!("get ready to hum....");
    for n in (1..=PREP_SECONDS).rev(){
        println!("{n}...");
        std::thread::sleep(Duration::from_secs(1));
    }

    //Recording+reading the result
    stream.play().expect("failed to start the stream");

    println!("recording for 5 seconds... hum something");
    std::thread::sleep(Duration::from_secs(RECORD_SECONDS));

    drop(stream);

    let recorded=samples.lock().unwrap();
    println!("captured {} interleaved samples", recorded.len());
    println!("that's {} frames per channel", recorded.len()/channels);
}
