use std::sync::{Arc,Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait,HostTrait,StreamTrait};
use midly::{Smf,TrackEventKind,MetaMessage,MidiMessage};
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
//Pitch cleaning
const VOICED_MAX_CMNDF: f32=0.30;
const NOTE_NAMES: [&str;12]=["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"];
//Median filter
const MEDIAN_WINDOW: usize=5;
//Note segmentation
const NOTE_CHANGE_ST: f32=1.0;
const MIN_NOTE_FRAMES: usize=5;
const MAX_GAP_FRAMES: usize=2;
//A jump this big between neighbouring notes is more likely an octave error
//than something a person hummed.
const OCTAVE_JUMP_ST: f32=6.0;


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

fn hz_to_semitones(hz: f32)->f32{
    12.0*(hz/440.0).log2()+69.0
}

fn clean_pitch_track(pitches: &[(f32,f32)])->Vec<Option<f32>>{
    pitches
    .iter()
    .map(|&(hz,confidence)|{
        if confidence<VOICED_MAX_CMNDF && hz>0.0{
            Some(hz_to_semitones(hz))
        }
        else{
            None
        }
    })
    .collect()
}

fn note_name(semitones: f32)->String{
    let midi=semitones.round() as i32;
    let name =NOTE_NAMES[midi.rem_euclid(12) as usize];
    let octave=midi.div_euclid(12)-1;
    format!("{name}{octave}")
}

//A single wrong frame becomes a fake note once we segment, so smooth first.
//Median not mean: the median only cares about ordering, so one octave-slipped
//frame out of five can never be the middle value no matter how far off it is.
fn median_filter(track: &[Option<f32>])->Vec<Option<f32>>{
    (0..track.len())
    .map(|i|{
        //Smooths pitch; never invents voicing where there was none.
        if track[i].is_none(){
            return None;
        }

        let lo=i.saturating_sub(MEDIAN_WINDOW/2);
        let hi=(i+MEDIAN_WINDOW/2+1).min(track.len());

        let mut window: Vec<f32>=track[lo..hi].iter().filter_map(|&n| n).collect();
        window.sort_by(f32::total_cmp);

        Some(window[window.len()/2])
    })
    .collect()
}

//One sung note: its pitch, and which frames it covered.
struct Note{
    semitones: f32,
    start: usize,
    end: usize,
}

//Median of the frames collected so far, not the mean -- same reason as the
//median filter. Drops anything too short to have been sung on purpose.
fn push_note(notes: &mut Vec<Note>,current: &mut Vec<f32>,start: usize,end: usize){
    if current.len()>=MIN_NOTE_FRAMES{
        current.sort_by(f32::total_cmp);
        notes.push(Note{
            semitones: current[current.len()/2],
            start,
            end,
        });
    }
    current.clear();
}

//Walk the smoothed track and cut it into notes. A note ends when the pitch
//moves away from where the note has been sitting, or when the voice stops for
//long enough that it was not just a dropped frame.
fn segment_notes(track: &[Option<f32>])->Vec<Note>{
    let mut notes=Vec::new();
    let mut current: Vec<f32>=Vec::new();
    let mut start=0;
    let mut end=0;
    let mut gap=0;

    for (i,frame) in track.iter().copied().enumerate(){
        match frame{
            Some(p)=>{
                if current.is_empty(){
                    start=i;
                }
                else{
                    //Compare against the note's running average, not the last
                    //frame, so slow drift inside a held note does not add up
                    //into a false boundary.
                    let mean=current.iter().sum::<f32>() / current.len() as f32;
                    if (p-mean).abs()>NOTE_CHANGE_ST{
                        push_note(&mut notes,&mut current,start,end);
                        start=i;
                    }
                }

                current.push(p);
                end=i;
                gap=0;
            }
            None=>{
                if !current.is_empty(){
                    gap+=1;
                    if gap>MAX_GAP_FRAMES{
                        push_note(&mut notes,&mut current,start,end);
                    }
                }
            }
        }
    }

    //Whatever is still open when the clip ends is a note too.
    push_note(&mut notes,&mut current,start,end);
    notes
}

//YIN sometimes locks onto half the true period, which reads as the right note
//an octave too high. It shows up as a lone note ~12 semitones off its
//neighbours that immediately returns. Fold those back toward the local line.
//Heuristic: real hummed leaps bigger than OCTAVE_JUMP_ST are rare.
fn fix_octaves(notes: &mut [Note])->usize{
    let mut fixed=0;

    for i in 0..notes.len(){
        let mut context: Vec<f32>=Vec::new();
        if i>0{ context.push(notes[i-1].semitones); }
        if i+1<notes.len(){ context.push(notes[i+1].semitones); }
        if context.is_empty(){ continue; }

        let local=context.iter().sum::<f32>() / context.len() as f32;
        let p=notes[i].semitones;

        if (p-local).abs()>OCTAVE_JUMP_ST{
            let shifted=if p>local{ p-12.0 } else { p+12.0 };
            if (shifted-local).abs()<(p-local).abs(){
                notes[i].semitones=shifted;
                fixed+=1;
            }
        }
    }

    fixed
}

//A melody is the gaps between notes, not the notes themselves. Sing it in any
//key and every note changes; every gap stays the same. This is what makes
//matching possible at all.
fn intervals(pitches: &[f32])->Vec<f32>{
    pitches
    .windows(2)
    .map(|pair| pair[1]-pair[0])
    .collect()
}

//--- MIDI side: the reference melodies we match against --------------------

#[derive(Clone)]
struct MidiNote{
    key: u8,
    start: u32,
    end: u32,
}

struct TrackInfo{
    index: usize,
    name: String,
    channel: u8,
    notes: Vec<MidiNote>,
    overlap: f32,
    mean_key: f32,
}

//A midi track is a stream of note-on/note-off events separated by delta times.
//Running totals turn deltas into absolute ticks; pairing each note-on with its
//matching note-off turns events into notes.
fn load_midi_tracks(path: &str)->Vec<TrackInfo>{
    let bytes=std::fs::read(path).expect("could not read midi file");
    let smf=Smf::parse(&bytes).expect("could not parse midi file");
    let mut infos=Vec::new();

    for (index,track) in smf.tracks.iter().enumerate(){
        let mut name=String::new();
        let mut channel: Option<u8>=None;
        let mut open: Vec<(u8,u32)>=Vec::new();
        let mut notes: Vec<MidiNote>=Vec::new();
        let mut tick: u32=0;

        for ev in track{
            tick+=ev.delta.as_int();

            match ev.kind{
                TrackEventKind::Meta(MetaMessage::TrackName(n))=>{
                    name=String::from_utf8_lossy(n).trim().to_string();
                }
                TrackEventKind::Midi{channel: ch,message}=>{
                    channel.get_or_insert(ch.as_int());

                    match message{
                        //Velocity 0 on a note-on means note-off, by convention.
                        MidiMessage::NoteOn{key,vel} if vel.as_int()>0=>{
                            open.push((key.as_int(),tick));
                        }
                        MidiMessage::NoteOn{key,..}|MidiMessage::NoteOff{key,..}=>{
                            let k=key.as_int();
                            if let Some(pos)=open.iter().position(|&(ok,_)| ok==k){
                                let (_,start)=open.remove(pos);
                                notes.push(MidiNote{key:k,start,end:tick});
                            }
                        }
                        _=>{}
                    }
                }
                _=>{}
            }
        }

        notes.sort_by_key(|n| n.start);

        //How often does a note start before the previous one ended? A sung
        //melody is nearly monophonic; chords, pads and drums are not.
        let overlaps=notes.windows(2).filter(|w| w[1].start<w[0].end).count();
        let overlap=if notes.len()>1{ overlaps as f32/(notes.len()-1) as f32 } else { 1.0 };
        let mean_key=if notes.is_empty(){ 0.0 } else {
            notes.iter().map(|n| n.key as f32).sum::<f32>() / notes.len() as f32
        };

        infos.push(TrackInfo{
            index,
            name,
            channel: channel.unwrap_or(255),
            notes,
            overlap,
            mean_key,
        });
    }

    infos
}

//Nothing in a midi file labels the melody, so guess: long enough to be a tune,
//not channel 9 (drums), inside a range a person could sing, and as close to
//monophonic as we can find.
fn pick_melody_track(tracks: &[TrackInfo])->Option<&TrackInfo>{
    tracks
    .iter()
    .filter(|t| t.notes.len()>=8)
    .filter(|t| t.channel!=9)
    .filter(|t| t.mean_key>=45.0 && t.mean_key<=84.0)
    .min_by(|a,b| a.overlap.total_cmp(&b.overlap))
}

//Flatten whatever overlap remains into a single line. The melody is normally
//the top voice, so when two notes sound together keep the higher one.
fn melody_line(notes: &[MidiNote])->Vec<f32>{
    let mut line: Vec<MidiNote>=Vec::new();

    for n in notes{
        match line.last_mut(){
            Some(prev) if n.start<prev.end=>{
                if n.key>prev.key{ *prev=n.clone(); }
            }
            _=>line.push(n.clone()),
        }
    }

    line.iter().map(|n| n.key as f32).collect()
}

fn inspect_midi(path: &str){
    let tracks=load_midi_tracks(path);

    println!("{path}: {} tracks",tracks.len());
    println!("  {:<3} {:<26} {:>3} {:>6} {:>9} {:>8}","#","name","ch","notes","mean key","overlap");

    for t in &tracks{
        println!(
            "  {:<3} {:<26} {:>3} {:>6} {:>9.1} {:>7.0}%",
            t.index,
            if t.name.is_empty(){ "-" } else { t.name.as_str() },
            t.channel,
            t.notes.len(),
            t.mean_key,
            t.overlap*100.0,
        );
    }

    match pick_melody_track(&tracks){
        Some(t)=>{
            let line=melody_line(&t.notes);
            let steps=intervals(&line);

            println!("--- melody guess: track {} \"{}\" ({} notes) ---",t.index,t.name,line.len());

            let names: Vec<String>=line.iter().take(24).map(|&k| note_name(k)).collect();
            println!("first notes:     {}",names.join(" "));

            let out: Vec<String>=steps.iter().take(40).map(|d| format!("{d:+.0}")).collect();
            println!("first intervals: {}",out.join(" "));
        }
        None=>println!("no track looked like a singable melody"),
    }
}

fn main(){
    //`cargo run -- midi/balada-gusttavo-lima.mid` inspects a midi file instead
    //of recording, so the reference side can be worked on without humming.
    if let Some(path)=std::env::args().nth(1){
        inspect_midi(&path);
        return;
    }

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
    let raw=clean_pitch_track(&pitches);
    let track=median_filter(&raw);

    let smoothed=raw.iter().zip(track.iter())
    .filter(|(a,b)| match (a,b){
        (Some(x),Some(y))=>(x-y).abs()>0.5,
        _=>false,
    })
    .count();
    println!("median filter changed {smoothed} frames");

    let voiced=track.iter().filter(|n| n.is_some()).count();
    println!("{voiced} of {} frames voiced",track.len());

    //Per-frame dump -- useful while tuning the pitch track, far too noisy once
    //segmentation works. Uncomment to debug a bad take.
    // for (i,note) in track.iter().enumerate(){
    //     match note{
    //         Some(s)=>println!("frame {i}: {s:.2} st  ({})",note_name(*s)),
    //         None=>println!("frame {i}: --"),
    //     }
    // }

    let mut notes=segment_notes(&track);
    let fixed=fix_octaves(&mut notes);
    println!("octave-corrected {fixed} notes");

    let seconds=|f: usize| f as f32 * HOP_SIZE as f32 / sample_rate as f32;

    println!("--- {} notes ---",notes.len());
    for (i,n) in notes.iter().enumerate(){
        println!(
            "note {i}: {:<4} {:6.2} st   frames {:>3}-{:<3}  {:.2}s -> {:.2}s",
            note_name(n.semitones),
            n.semitones,
            n.start,
            n.end,
            seconds(n.start),
            seconds(n.end+1),
        );
    }

    let pitches: Vec<f32>=notes.iter().map(|n| n.semitones).collect();
    let steps=intervals(&pitches);
    println!("--- {} intervals (semitones between consecutive notes) ---",steps.len());
    let line: Vec<String>=steps.iter().map(|d| format!("{d:+.1}")).collect();
    println!("{}",line.join(" "));
}