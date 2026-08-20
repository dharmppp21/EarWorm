use std::sync::{Arc,Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait,HostTrait,StreamTrait};
// use rustfft::{FftPlanner, num_complex::Complex}; // only needed by the commented-out spectrogram below

//PrepTime
const PREP_SECONDS: u64=3;
const EXPECTED_MAX_SECONDS: u64=30;
//FFT
const WINDOW_SIZE: usize=4096;
const HOP_SIZE: usize=1024;
//Autocorrelation
const MIN_FREQ_HZ: f32=80.0;
const MAX_FREQ_HZ: f32=1400.0;
//YIN
const YIN_THRESHOLD: f32=0.25;
//Mic priority: earlier = better. A mic near your mouth beats a mic across the room.
const MIC_PREFER: [&str;3]=["jack","earbud","headphone"];
//Ranks below every unlisted device; earlier = less bad. Bluetooth headset mics
//negotiate 16 kHz and often stream nothing at all, so they go last.
const MIC_DEMOTE: [&str;2]=["array","headset"];
const MIC_REJECT: [&str;2]=["stereo mix","what u hear"];


fn pick_input_device(host: &cpal::Host)->cpal::Device{
    let mut best: Option<(usize,cpal::Device)>=None;

    for device in host.input_devices().expect("failed to list input devices"){
        let name=device.to_string().to_lowercase();

        if MIC_REJECT.iter().any(|bad| name.contains(bad)){
            println!("  skip  {device} (loopback, not a real mic)");
            continue;
        }

        let rank=if let Some(i)=MIC_PREFER.iter().position(|want| name.contains(want)){
            i
        }else if let Some(i)=MIC_DEMOTE.iter().position(|d| name.contains(d)){
            MIC_PREFER.len()+1+i
        }else{
            MIC_PREFER.len()
        };

        println!("  found {device} (priority {rank})");

        if best.as_ref().map_or(true,|(r,_)| rank<*r){
            best=Some((rank,device));
        }
    }

    best.map(|(_,d)| d)
    .or_else(|| host.default_input_device())
    .expect("no input device found")
}

fn record(expected_max_seconds: u64)->(Vec<f32>,u32){
    //Device Config
    let host=cpal::default_host();
    let device=pick_input_device(&host);
    let config=device.default_input_config().expect("no default config");

    let sample_rate=config.sample_rate();
    let channels=config.channels() as usize;
    println!("recording from {device} at {sample_rate} Hz, {channels} channel(s)");

    //Shared Buffer
    let expected_samples=sample_rate as usize*channels*expected_max_seconds as usize;
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
    println!("recording now! hum something, then press enter when you are done");
    
    let mut input=String::new();
    std::io::stdin().read_line(&mut input).expect("failed to read input");

    drop(stream);

    let interleaved=samples.lock().unwrap();
    let mono: Vec<f32>=interleaved
    .chunks_exact(channels)
    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
    .collect();

    (mono,sample_rate)
}

// Superseded by autocorrelation-based pitch detection below (track_pitch /
// detect_pitch_naive) -- kept for reference, not called by main() anymore.
// Loudest-FFT-bin picking a harmonic instead of the fundamental, and getting
// dominated by background audio, is exactly what motivated the switch.

// fn hann_window(size: usize)->Vec<f32>{
//     (0..size)
//     .map(|n|{
//         0.5*(1.0 - (2.0 * std::f32::consts::PI * n as f32 / (size-1) as f32).cos())
//     })
//     .collect()
// }

// fn spectogram(samples: &[f32])->Vec<Vec<f32>>{
//     let window=hann_window(WINDOW_SIZE);
//     let mut planner=FftPlanner::new();
//     let fft=planner.plan_fft_forward(WINDOW_SIZE);
//
//     let mut frames=Vec::new();
//     let mut start=0;
//     while start + WINDOW_SIZE<=samples.len(){
//         let mut buffer: Vec<Complex<f32>>=samples[start..start+WINDOW_SIZE]
//         .iter()
//         .zip(window.iter())
//         .map(|(&sample,&w)| Complex {re: sample*w,im: 0.0f32})
//         .collect();
//
//         fft.process(&mut buffer);
//
//         let half=WINDOW_SIZE/2;
//         let magnitudes: Vec<f32>=buffer[..=half]
//         .iter()
//         .map(|c| (c.re*c.re + c.im*c.im).sqrt())
//         .collect();
//
//         frames.push(magnitudes);
//         start+=HOP_SIZE;
//     }
//
//     frames
// }

// Superseded by YIN (detect_pitch_yin below) -- kept for reference, not
// called by main() anymore. Raw dot-product autocorrelation is structurally
// biased toward short lags (fewer overlapping terms as lag grows), which
// showed up as results repeatedly snapping to the two search-range boundaries
// instead of a real pitch. YIN's normalized difference function fixes that.

// fn autocorrelate_at_lag(frame: &[f32], lag: usize)->f32{
//     let n=frame.len()-lag;
//     let mut sum=0.0;
//     for i in 0..n{
//         sum+=frame[i]*frame[i+lag];
//     }
//     sum
// }
//
// fn detect_pitch_naive(frame: &[f32],sample_rate:u32)->f32{
//     let min_lag=(sample_rate as f32 / MAX_FREQ_HZ) as usize;
//     let max_lag=(sample_rate as f32 / MIN_FREQ_HZ) as usize;
//
//     let mut best_lag=min_lag;
//     let mut best_score=f32::MIN;
//
//     for lag in min_lag..=max_lag{
//         let score=autocorrelate_at_lag(frame,lag);
//         if score>best_score{
//             best_score=score;
//             best_lag=lag;
//         }
//     }
//
//     sample_rate as f32 / best_lag as f32
// }

//Slide same window for spectogram but estimate pitch not loudness per bin
fn track_pitch(samples: &[f32],sample_rate:u32)->Vec<(f32,f32)>{
    let mut pitches=Vec::new();
    let mut start=0;
    while start+WINDOW_SIZE<=samples.len(){
        let frame= &samples[start..start + WINDOW_SIZE];

        pitches.push(detect_pitch_yin(frame,sample_rate));
        start+=HOP_SIZE;
    }
    pitches
}

fn difference_at_lag(frame: &[f32], lag: usize)->f32{
    let n=frame.len()-lag;
    let mut sum=0.0;
    for i in 0..n{
        let diff=frame[i]-frame[i+lag];
        sum+=diff*diff;
    }
    sum
}

fn detect_pitch_yin(frame: &[f32], sample_rate: u32)->(f32,f32){
    let min_lag=(sample_rate as f32 / MAX_FREQ_HZ) as usize;
    let max_lag=(sample_rate as f32 / MIN_FREQ_HZ) as usize;

    let mut d=vec![0.0f32; max_lag+1];
    for lag in 1..=max_lag{
        d[lag]=difference_at_lag(frame,lag);
    }

    let mut cmndf=vec![1.0f32; max_lag+1];
    let mut running_sum=0.0;
    for lag in 1..=max_lag{
        running_sum+=d[lag];
        cmndf[lag]=if running_sum>0.0{ d[lag]* lag as f32 / running_sum} else {1.0};
    }

    for lag in min_lag..=max_lag{
        if cmndf[lag]<YIN_THRESHOLD{
            let mut best=lag;
            while best+1<=max_lag && cmndf[best+1]<cmndf[best]{
                best+=1;
            }
            return (sample_rate as f32 / best as f32,cmndf[best]);
        }
    }

    let best_lag=(min_lag..=max_lag)
    .min_by(|&a,&b| cmndf[a].total_cmp(&cmndf[b]))
    .unwrap();
    
    (sample_rate as f32 / best_lag as f32,cmndf[best_lag])
}

fn main(){
    let (clip,sample_rate)=record(EXPECTED_MAX_SECONDS);
    println!(
        "mono clip: {} samples at {} Hz ({:.2}s)",
        clip.len(),
        sample_rate,
        clip.len() as f32 / sample_rate as f32
    );

    if clip.is_empty(){
        eprintln!("no audio captured -- the selected device produced nothing");
        return;
    }

    let rms=(clip.iter().map(|s| s*s).sum::<f32>() / clip.len() as f32).sqrt();
    let peak=clip.iter().fold(0.0f32,|m,&s| m.max(s.abs()));
    println!("level: rms={rms:.4} peak={peak:.4}");

    if rms<0.01{
        eprintln!("warning: level too low, pitch detection will be unreliable");
    }

    let pitches=track_pitch(&clip,sample_rate);
    for (i,(hz,confidence)) in pitches.iter().enumerate(){
        println!("frame {i}: pitch ~={hz:.1} Hz (confidence={confidence:.3})");
    }
}