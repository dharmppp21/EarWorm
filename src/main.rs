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
//Matching: anything smaller than this is a repeated note, not a move.
const MIN_MOVE_ST: f32=0.75;
//Measured against this library: 7 accurate moves identify a song at ~6.7x
//margin, 12 at ~24x, 22 at ~56x. A hum is never accurate, so it needs the
//extra length to make up for it. Below this, expect nothing.
const MIN_USEFUL_MOVES: usize=12;
//Every take is kept here. Without fixed audio to re-run, tuning the front end
//changes the code and the performance at once and neither can be blamed.
const TAKES_DIR: &str="takes";


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

    //Start the stream *before* the countdown and throw away everything it
    //captures during it. The first callback after play() has to fault in code
    //pages and take the mutex for the first time, which takes longer than the
    //device buffer lasts -- that is the xrun cpal reports. Warming up during
    //the countdown puts that cost where nobody is singing yet.
    stream.play().expect("failed to start the stream");

    //PrepTime
    println!("get ready to hum....");
    for n in (1..=PREP_SECONDS).rev(){
        println!("{n}...");
        std::thread::sleep(Duration::from_secs(1));
    }

    //Recording+reading the result
    samples.lock().unwrap().clear();
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

//What instrument each track is set to, which is how you tell a backing
//arrangement from one that carries the tune.
fn track_instruments(path: &str)->std::collections::HashMap<usize,String>{
    let bytes=std::fs::read(path).expect("could not read midi file");
    let smf=Smf::parse(&bytes).expect("could not parse midi file");
    let mut out=std::collections::HashMap::new();

    for (i,track) in smf.tracks.iter().enumerate(){
        let mut progs: Vec<u8>=Vec::new();

        for ev in track{
            if let TrackEventKind::Midi{message: MidiMessage::ProgramChange{program},..}=ev.kind{
                let p=program.as_int();
                if !progs.contains(&p){ progs.push(p); }
            }
        }

        if !progs.is_empty(){
            let names: Vec<String>=progs.iter()
            .map(|&p| format!("{}({p})",GM_FAMILY[(p/8) as usize]))
            .collect();
            out.insert(i,names.join(","));
        }
    }

    out
}

fn inspect_midi(path: &str){
    let tracks=load_midi_tracks(path);
    let instruments=track_instruments(path);

    println!("{path}: {} tracks",tracks.len());
    println!("  {:<3} {:<18} {:<20} {:>3} {:>6} {:>9} {:>8}","#","name","instrument","ch","notes","mean key","overlap");

    for t in &tracks{
        println!(
            "  {:<3} {:<18} {:<20} {:>3} {:>6} {:>9.1} {:>7.0}%",
            t.index,
            if t.name.is_empty(){ "-" } else { t.name.as_str() },
            instruments.get(&t.index).map(|s| s.as_str()).unwrap_or("-"),
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

//--- Matching --------------------------------------------------------------

//Dynamic time warping: the cheapest way to line two sequences up when one has
//extra or missing entries relative to the other. Every cell asks the same
//question -- what is the cheapest way to have arrived here -- by taking the
//best of three moves: consume one from a, one from b, or one from each.
fn dtw(a: &[f32],b: &[f32])->f32{
    if a.is_empty() || b.is_empty(){
        return f32::INFINITY;
    }

    let n=a.len();
    let m=b.len();

    //Only the previous row is ever needed, so keep two rows instead of an
    //n*m grid. Same answer, a fraction of the memory.
    let mut prev=vec![f32::INFINITY; m+1];
    let mut curr=vec![f32::INFINITY; m+1];
    prev[0]=0.0;

    for i in 1..=n{
        curr[0]=f32::INFINITY;

        for j in 1..=m{
            let cost=(a[i-1]-b[j-1]).abs();
            let best=prev[j].min(curr[j-1]).min(prev[j-1]);
            curr[j]=cost+best;
        }

        std::mem::swap(&mut prev,&mut curr);
    }

    //Normalise by path length so sequences of different sizes compare fairly.
    prev[m] / (n+m) as f32
}

//Subsequence DTW: the query has to match somewhere *inside* the reference,
//not stretch across the whole of it. Two changes from plain DTW. The first row
//is zero everywhere, so beginning at any point in the reference costs nothing.
//And the answer is the best cell in the last row rather than the far corner,
//so ending anywhere costs nothing either.
//Returns the score plus where in the reference the match sits.
fn subsequence_dtw(query: &[f32],reference: &[f32])->(f32,usize,usize){
    if query.is_empty() || reference.len()<query.len(){
        return (f32::INFINITY,0,0);
    }

    let n=query.len();
    let m=reference.len();

    let mut prev=vec![0.0f32; m+1];
    let mut curr=vec![f32::INFINITY; m+1];

    //Each cell carries the reference position its path began at, so the match
    //can be located without storing the whole n*m grid to backtrack through.
    let mut prev_start: Vec<usize>=(0..=m).collect();
    let mut curr_start=vec![0usize; m+1];

    for i in 1..=n{
        curr[0]=f32::INFINITY;
        curr_start[0]=0;

        for j in 1..=m{
            let cost=(query[i-1]-reference[j-1]).abs();

            //Step through both, skip a reference note, or skip a query note --
            //whichever arrived here cheapest, inheriting where it started.
            let mut best=prev[j-1];
            let mut from=prev_start[j-1];

            if prev[j]<best{ best=prev[j]; from=prev_start[j]; }
            if curr[j-1]<best{ best=curr[j-1]; from=curr_start[j-1]; }

            curr[j]=cost+best;
            curr_start[j]=from;
        }

        std::mem::swap(&mut prev,&mut curr);
        std::mem::swap(&mut prev_start,&mut curr_start);
    }

    //Best place to have finished, normalised by how much ground the path
    //covered so a short accidental alignment cannot undercut a long real one.
    let mut best_score=f32::INFINITY;
    let mut best_j=1;

    for j in 1..=m{
        let span=(j-prev_start[j]).max(1);
        let score=prev[j] / (n+span) as f32;

        if score<best_score{
            best_score=score;
            best_j=j;
        }
    }

    (best_score,prev_start[best_j],best_j-1)
}

//Matching fails quietly -- a wrong implementation still returns a confident
//number. These cases have known answers, so they catch that.
fn selftest(){
    let notes=[60.0f32,62.0,64.0,62.0,60.0,67.0,65.0,64.0];
    let a=intervals(&notes);

    //Intervals are differences, so moving every note up a fifth changes
    //nothing. This must be exactly zero.
    let up: Vec<f32>=notes.iter().map(|n| n+7.0).collect();
    let a_up=intervals(&up);

    //Sung a little flat and sharp in turn.
    let sloppy: Vec<f32>=a.iter().enumerate()
    .map(|(i,&x)| x + if i%2==0{ 0.3 } else { -0.25 })
    .collect();

    //Warping earns its keep on missed and added notes. Tempo is already
    //absorbed -- note-level intervals do not care how long a note was held.
    let mut dropped=notes.to_vec();
    dropped.remove(4);
    let a_dropped=intervals(&dropped);

    let mut extra=notes.to_vec();
    extra.insert(4,63.0);
    let a_extra=intervals(&extra);

    let other=intervals(&[60.0f32,55.0,67.0,58.0,70.0,52.0,63.0,49.0]);

    println!("dtw self-test  (lower = better match)");
    println!("  identical              {:.4}",dtw(&a,&a));
    println!("  transposed up a fifth  {:.4}",dtw(&a,&a_up));
    println!("  sung slightly off      {:.4}",dtw(&a,&sloppy));
    println!("  one note missed        {:.4}",dtw(&a,&a_dropped));
    println!("  one note added         {:.4}",dtw(&a,&a_extra));
    println!("  unrelated melody       {:.4}",dtw(&a,&other));

    //Subsequence: plant a known melody inside a longer sequence and check it
    //is found, in the right place, and not found when it is absent.
    let filler=[3.0f32,-5.0,1.0,7.0,-2.0,-3.0,4.0,6.0,-7.0,2.0,-4.0,5.0];
    let mut haystack=filler.to_vec();
    let planted=haystack.len();
    haystack.extend_from_slice(&a);
    haystack.extend_from_slice(&filler);

    println!("subsequence self-test  (melody planted at {planted})");

    for (label,q) in [("exact",&a),("sung off",&sloppy),("absent",&other)]{
        let (cost,start,end)=subsequence_dtw(q,&haystack);
        println!("  {label:<9} cost {cost:.4}  found at {start}-{end}");
    }
}

//32-bit float, so saving and reloading a clip changes nothing about it.
fn save_take(clip: &[f32],sample_rate: u32)->Option<String>{
    if clip.is_empty(){
        return None;
    }

    std::fs::create_dir_all(TAKES_DIR).ok()?;

    //Next free number, so takes accumulate into a test set instead of
    //overwriting each other.
    let mut n=1;
    while std::path::Path::new(&format!("{TAKES_DIR}/take-{n}.wav")).exists(){
        n+=1;
    }
    let path=format!("{TAKES_DIR}/take-{n}.wav");

    let spec=hound::WavSpec{
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer=hound::WavWriter::create(&path,spec).ok()?;
    for &s in clip{
        writer.write_sample(s).ok()?;
    }
    writer.finalize().ok()?;

    Some(path)
}

//Accepts any wav -- int or float, any depth, any channel count -- and
//normalises to the same mono f32 that record() produces.
fn load_wav(path: &str)->Result<(Vec<f32>,u32),hound::Error>{
    let mut reader=hound::WavReader::open(path)?;
    let spec=reader.spec();
    let channels=spec.channels as usize;

    let interleaved: Vec<f32>=match spec.sample_format{
        hound::SampleFormat::Float=>{
            reader.samples::<f32>().collect::<Result<Vec<_>,_>>()?
        }
        hound::SampleFormat::Int=>{
            let scale=(1i64<<(spec.bits_per_sample-1)) as f32;
            reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / scale))
            .collect::<Result<Vec<_>,_>>()?
        }
    };

    let mono: Vec<f32>=interleaved
    .chunks_exact(channels)
    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
    .collect();

    Ok((mono,spec.sample_rate))
}

//Either a fresh performance or a saved one. Passing a wav is what makes
//tuning measurable: the input is frozen, so any change in output came from
//the code.
fn clip_from(source: Option<&str>)->(Vec<f32>,u32){
    match source{
        Some(path)=>{
            match load_wav(path){
                Ok(loaded)=>{
                    println!("loaded {path}");
                    loaded
                }
                Err(e)=>{
                    eprintln!("could not read {path}: {e}");
                    (Vec::new(),48000)
                }
            }
        }
        None=>{
            let (clip,sample_rate)=record(EXPECTED_MAX_SECONDS);
            if let Some(saved)=save_take(&clip,sample_rate){
                println!("saved {saved}");
            }
            (clip,sample_rate)
        }
    }
}

//Record, clean, segment, and reduce to the interval sequence -- the melody in
//the form everything downstream compares. Returns it so matching can use the
//same pipeline the plain run prints.
fn capture_intervals(source: Option<&str>)->Vec<f32>{
    let (clip,sample_rate)=clip_from(source);
    println!(
        "mono clip: {} samples at {} Hz ({:.2}s)",
        clip.len(),
        sample_rate,
        clip.len() as f32 / sample_rate as f32
    );

    if clip.is_empty(){
        eprintln!("no audio captured -- the selected device produced nothing");
        return Vec::new();
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

    steps
}

//Repeated notes carry almost no identifying information -- every track is full
//of them -- and a hum's come out as fuzzy fractions where a midi's are exact
//zeros, so comparing them directly just adds noise. Keep only the moves,
//rounded to whole semitones, which is the alphabet a midi is already written
//in. Measured on a real take: raw intervals separated the best track from the
//runner-up by 1.03x, rounding alone 1.22x, moves-only 1.61x.
fn melody_shape(steps: &[f32])->Vec<f32>{
    steps
    .iter()
    .copied()
    .filter(|d| d.abs()>=MIN_MOVE_ST)
    .map(|d| d.round())
    .collect()
}

//Render a midi track as tones so the whole chain -- audio, pitch detection,
//segmentation, intervals, matching -- can be tested end to end without a
//microphone or a singer. If this stops identifying its own song, something
//downstream of the mic has broken.
fn synth_track(path: &str,notes: usize)->Option<String>{
    let tracks=load_midi_tracks(path);
    //Same filter as pick_melody_track: the most monophonic track that a person
    //could actually sing. Without the range check this picks the bass, which
    //sits below MIN_FREQ_HZ and yields no pitch at all.
    let track=tracks.iter()
    .filter(|t| t.channel!=9 && t.notes.len()>=notes)
    .filter(|t| t.mean_key>=45.0 && t.mean_key<=84.0)
    .min_by(|a,b| a.overlap.total_cmp(&b.overlap))?;

    let line=melody_line(&track.notes);
    let sr=48000u32;

    let clip: Vec<f32>=line.iter().take(notes).enumerate().flat_map(|(i,&key)|{
        let hz=440.0*2f32.powf((key-69.0)/12.0);
        let n=(sr as f32*0.38) as usize;
        let mut state=(i as u32+1).wrapping_mul(2654435761);

        (0..n).map(move |k|{
            let t=k as f32/sr as f32;
            //Fade the edges so note boundaries are not clicks, which would
            //read as broadband noise and wreck the pitch estimate.
            let env=(t/0.03).min(1.0).min((0.38-t)/0.03).max(0.0);
            state=state.wrapping_mul(1103515245).wrapping_add(12345);
            let noise=(((state>>16) as f32/32768.0)-1.0)*0.01;
            let tp=2.0*std::f32::consts::PI;
            //A couple of harmonics, so it is not a bare sine.
            let v=(tp*hz*t).sin()+0.4*(tp*hz*2.0*t).sin()+0.2*(tp*hz*3.0*t).sin();
            v*0.25*env+noise
        }).collect::<Vec<f32>>()
    }).collect();

    std::fs::create_dir_all(TAKES_DIR).ok()?;
    let out=format!("{TAKES_DIR}/synth.wav");
    let spec=hound::WavSpec{
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut w=hound::WavWriter::create(&out,spec).ok()?;
    for &v in &clip{ w.write_sample(v).ok()?; }
    w.finalize().ok()?;

    println!("wrote {out}  ({:.1}s from track {} \"{}\")",
        clip.len() as f32/sr as f32,track.index,track.name);
    println!("notes: {}",line.iter().take(notes)
        .map(|&k| note_name(k)).collect::<Vec<_>>().join(" "));

    Some(out)
}

//Rank whole songs, not tracks: score every track in every file and let each
//song's best track stand for it. This is the question recognition actually
//asks -- with one file the matcher can only rank tracks inside it, and nothing
//is ever given the chance to lose.
//Assumes the folder holds valid midis; a corrupt one will panic in parsing.
fn match_against_library(dir: &str,query: &[f32]){
    let shape=melody_shape(query);

    if shape.len()<4{
        println!("only {} real moves -- hum a longer or less flat phrase",shape.len());
        return;
    }

    println!("hum shape ({} moves): {}",shape.len(),
        shape.iter().map(|d| format!("{d:+.0}")).collect::<Vec<_>>().join(" "));

    if shape.len()<MIN_USEFUL_MOVES{
        println!("WARNING: {} moves is below the {MIN_USEFUL_MOVES} needed for a trustworthy result -- a short contour fits almost any song, so treat whatever follows as noise",shape.len());
    }

    let entries=match std::fs::read_dir(dir){
        Ok(e)=>e,
        Err(e)=>{
            eprintln!("could not read {dir}: {e}");
            return;
        }
    };

    let mut songs: Vec<(f32,String,usize,usize,usize)>=Vec::new();

    for entry in entries.flatten(){
        let path=entry.path();

        if path.extension().and_then(|e| e.to_str())!=Some("mid"){
            continue;
        }

        let name=path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let tracks=load_midi_tracks(&path.to_string_lossy());
        let mut best: Option<(f32,usize,usize,usize)>=None;

        for t in &tracks{
            let refs=melody_shape(&intervals(&melody_line(&t.notes)));

            if refs.len()<shape.len(){
                continue;
            }

            let (cost,start,end)=subsequence_dtw(&shape,&refs);

            if best.map_or(true,|(b,_,_,_)| cost<b){
                best=Some((cost,t.index,start,end));
            }
        }

        if let Some((cost,track,start,end))=best{
            songs.push((cost,name,track,start,end));
        }
    }

    songs.sort_by(|a,b| a.0.total_cmp(&b.0));

    println!("--- {} songs ranked (lower = better) ---",songs.len());
    for (cost,name,track,start,end) in &songs{
        println!("  {cost:>7.4}  {name:<36} trk {track:<3} at {start}-{end}");
    }

    if songs.len()>=2{
        let best=songs[0].0;
        let next=songs[1].0;
        let margin=next / best.max(1e-6);

        println!("best {best:.4}, next {next:.4} -- {margin:.2}x margin");

        if margin<1.5{
            println!("verdict: no clear match, the field is bunched");
        }
        else{
            println!("verdict: {}",songs[0].1);
        }
    }
}

//Score the hum against every track and rank them. Which track wins answers
//"did I sing the vocal line or the synth hook" as a result instead of a guess.
//The gap between best and second is what says whether anything matched at all.
fn match_against_midi(path: &str,query: &[f32]){
    let shape=melody_shape(query);

    if shape.len()<4{
        println!("only {} real moves -- hum a longer or less flat phrase",shape.len());
        return;
    }

    println!("hum shape ({} moves): {}",shape.len(),
        shape.iter().map(|d| format!("{d:+.0}")).collect::<Vec<_>>().join(" "));

    let tracks=load_midi_tracks(path);
    let mut scored: Vec<(f32,usize,String,usize,usize,usize)>=Vec::new();

    for t in &tracks{
        let line=melody_line(&t.notes);
        let refs=melody_shape(&intervals(&line));

        if refs.len()<shape.len(){
            continue;
        }

        let (cost,start,end)=subsequence_dtw(&shape,&refs);
        let name=if t.name.is_empty(){ String::from("-") } else { t.name.clone() };
        scored.push((cost,t.index,name,refs.len(),start,end));
    }

    scored.sort_by(|a,b| a.0.total_cmp(&b.0));

    println!("--- match against {path} ---");
    println!("  {:>7}  {:<4} {:<14} {:>6}  {}","score","trk","name","moves","matched at");

    for (cost,index,name,len,start,end) in &scored{
        println!("  {cost:>7.4}  {index:<4} {name:<14} {len:>6}  {start}-{end}");
    }

    //A real match sits well clear of the field. Bunched scores mean nothing
    //was recognised, whatever the top row happens to say.
    if scored.len()>=2{
        let best=scored[0].0;
        let next=scored[1].0;
        let margin=next / best.max(1e-6);

        println!("best {best:.4}, next {next:.4} -- {margin:.1}x margin");

        if margin<1.5{
            println!("verdict: no clear match, the field is bunched");
        }
        else{
            println!("verdict: track {} is the best candidate",scored[0].1);
        }
    }
}

//General MIDI groups programs in eights. Knowing a track is a Bass or a Flute
//says more than its note statistics do -- a karaoke file with no lead vocal
//looks perfectly healthy until you read the instrument list.
const GM_FAMILY: [&str;16]=["Piano","ChromPerc","Organ","Guitar","Bass","Strings",
    "Ensemble","Brass","Reed","Pipe","SynthLead","SynthPad","SynthFX","Ethnic",
    "Percussive","SoundFX"];

//Sing the same phrase twice and compare. If two takes of one phrase disagree
//more than one of them disagrees with a stranger, nothing downstream can work,
//and the fault is here rather than in the matching.
fn compare_takes(){
    println!("take 1 -- sing your phrase");
    let a=melody_shape(&capture_intervals(None));

    println!();
    println!("take 2 -- sing the SAME phrase again, the same way");
    let b=melody_shape(&capture_intervals(None));

    println!();
    println!("take 1 ({} moves): {}",a.len(),
        a.iter().map(|d| format!("{d:+.0}")).collect::<Vec<_>>().join(" "));
    println!("take 2 ({} moves): {}",b.len(),
        b.iter().map(|d| format!("{d:+.0}")).collect::<Vec<_>>().join(" "));

    if a.len()<3 || b.len()<3{
        println!("not enough moves to compare");
        return;
    }

    let full=dtw(&a,&b);

    //Subsequence needs the shorter side as the query, otherwise there is no
    //window long enough to hold it and the answer is infinity.
    let (sub,st,en)=if a.len()<=b.len(){
        subsequence_dtw(&a,&b)
    }
    else{
        subsequence_dtw(&b,&a)
    };

    println!("agreement between takes  {full:.4}   (best window {sub:.4} at {st}-{en})");
    println!("for reference, an unrelated shape scores about 1.0");

    if full<0.35{
        println!("verdict: reproducible -- the front end is fine");
    }
    else if full<0.7{
        println!("verdict: usable but loose");
    }
    else{
        println!("verdict: NOT reproducible -- matching cannot work until this improves");
    }
}

//Take a slice of one song's own melody and ask the library which song it is.
//A system that works must rank that song first, by a wide margin. No mic
//needed, so this is the regression test for matching.
fn verify(dir: &str){
    let mut files: Vec<std::path::PathBuf>=std::fs::read_dir(dir).expect("dir")
        .flatten().map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str())==Some("mid"))
        .collect();
    files.sort();

    for probe in &files{
        let tracks=load_midi_tracks(&probe.to_string_lossy());
        let best_track=tracks.iter()
            .filter(|t| t.channel!=9 && t.notes.len()>=40)
            .min_by(|a,b| a.overlap.total_cmp(&b.overlap));
        let Some(bt)=best_track else{ continue; };

        let full=melody_shape(&intervals(&melody_line(&bt.notes)));
        if full.len()<30{ continue; }

        //A 20-move slice from the middle, like humming one section -- then
        //damaged the way a real hum is: two intervals off by a semitone, one
        //note missed entirely. An exact copy of the melody would only prove
        //the file loader works.
        let mut query: Vec<f32>=full[full.len()/3..full.len()/3+20].to_vec();
        query[3]+=1.0;
        query[11]-=1.0;
        query.remove(7);

        let mut scored: Vec<(f32,String)>=Vec::new();
        for f in &files{
            let ts=load_midi_tracks(&f.to_string_lossy());
            let mut b=f32::INFINITY;
            for t in &ts{
                let refs=melody_shape(&intervals(&melody_line(&t.notes)));
                if refs.len()<query.len(){ continue; }
                let (c,_,_)=subsequence_dtw(&query,&refs);
                if c<b{ b=c; }
            }
            if b.is_finite(){
                scored.push((b,f.file_stem().unwrap().to_string_lossy().to_string()));
            }
        }
        scored.sort_by(|a,b| a.0.total_cmp(&b.0));

        let want=probe.file_stem().unwrap().to_string_lossy().to_string();
        let hit=scored[0].1==want;
        let margin=if scored.len()>1 && scored[0].0>1e-4{ scored[1].0/scored[0].0 } else { f32::INFINITY };
        println!("{} {:<34} -> {:<34} {:.4} vs {:.4}  {:.1}x",
            if hit{"PASS"}else{"FAIL"},want,scored[0].1,scored[0].0,scored[1].0,margin);
    }
}

fn main(){
    let args: Vec<String>=std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()){
        //`cargo run -- selftest`               check the matcher against
        //                                      cases with known answers
        //`cargo run -- match <file.mid>`       hum, then score every track
        //`cargo run -- <file.mid>`             inspect a midi without humming
        //`cargo run`                           hum and print the melody
        Some("selftest")=>selftest(),
        Some("verify")=>verify("midi"),
        Some("synth")=>{
            match args.get(1){
                Some(path)=>{ synth_track(path,30); }
                None=>eprintln!("usage: cargo run -- synth <file.mid>"),
            }
        }
        Some("compare")=>compare_takes(),
        Some("match")=>{
            match args.get(1){
                Some(path)=>{
                    let query=capture_intervals(args.get(2).map(|s| s.as_str()));

                    //A folder means "which song is this"; a file means "which
                    //track of this song did I sing".
                    if std::path::Path::new(path).is_dir(){
                        match_against_library(path,&query);
                    }
                    else{
                        match_against_midi(path,&query);
                    }
                }
                None=>eprintln!("usage: cargo run -- match <file.mid | folder> [take.wav]"),
            }
        }
        Some(path)=>inspect_midi(path),
        None=>{ capture_intervals(None); }
    }
}
