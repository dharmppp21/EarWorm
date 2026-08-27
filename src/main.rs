use std::sync::{Arc,Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait,HostTrait,StreamTrait};
use midly::{Smf,TrackEventKind,MetaMessage,MidiMessage};

use earworm::*;

const PREP_SECONDS: u64=3;

const EXPECTED_MAX_SECONDS: u64=30;

const MIC_PREFER: [&str;3]=["jack","earbud","headphone"];

const MIC_DEMOTE: [&str;2]=["array","headset"];

const MIC_REJECT: [&str;2]=["stereo mix","what u hear"];

const TAKES_DIR: &str="takes";

const HUMS_FILE: &str="hums.txt";

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
    let host=cpal::default_host();
    let device=pick_input_device(&host);
    let config=device.default_input_config().expect("no default config");

    let sample_rate=config.sample_rate();
    let channels=config.channels() as usize;
    println!("recording from {device} at {sample_rate} Hz, {channels} channel(s)");

    let expected_samples=sample_rate as usize*channels*expected_max_seconds as usize;
    let samples=Arc::new(Mutex::new(Vec::<f32>::with_capacity(expected_samples)));
    let samples_for_callback=samples.clone();

    let err_fn=move |err|{
        eprintln!("stream error: {err}");
    };

    let stream=device.build_input_stream(
        config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo|{
            samples_for_callback.lock().unwrap().extend_from_slice(data);
        },
        err_fn,
        None,
    )
    .expect("failed to build input stream");

    stream.play().expect("failed to start the stream");

    println!("get ready to hum....");
    for n in (1..=PREP_SECONDS).rev(){
        println!("{n}...");
        std::thread::sleep(Duration::from_secs(1));
    }

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

fn load_midi_tracks(path: &str)->Vec<TrackInfo>{
    let bytes=std::fs::read(path).expect("could not read midi file");
    let smf=Smf::parse(&bytes).expect("could not parse midi file");
    let mut infos=Vec::new();

    for track in &smf.tracks{
        let mut name=String::new();
        let mut open: Vec<(u8,u8,u32)>=Vec::new();
        let mut by_channel: std::collections::BTreeMap<u8,Vec<MidiNote>>=Default::default();
        let mut tick: u32=0;

        for ev in track{
            tick+=ev.delta.as_int();

            match ev.kind{
                TrackEventKind::Meta(MetaMessage::TrackName(n))=>{
                    name=String::from_utf8_lossy(n).trim().to_string();
                }
                TrackEventKind::Midi{channel,message}=>{
                    let ch=channel.as_int();

                    match message{
                        MidiMessage::NoteOn{key,vel} if vel.as_int()>0=>{
                            open.push((ch,key.as_int(),tick));
                        }
                        MidiMessage::NoteOn{key,..}|MidiMessage::NoteOff{key,..}=>{
                            let k=key.as_int();
                            if let Some(pos)=open.iter().position(|&(c,ok,_)| c==ch && ok==k){
                                let (_,_,start)=open.remove(pos);
                                by_channel.entry(ch).or_default()
                                .push(MidiNote{key:k,start,end:tick});
                            }
                        }
                        _=>{}
                    }
                }
                _=>{}
            }
        }

        for (channel,mut notes) in by_channel{
            notes.sort_by_key(|n| n.start);

            let overlaps=notes.windows(2).filter(|w| w[1].start<w[0].end).count();
            let overlap=if notes.len()>1{ overlaps as f32/(notes.len()-1) as f32 } else { 1.0 };
            let mean_key=if notes.is_empty(){ 0.0 } else {
                notes.iter().map(|n| n.key as f32).sum::<f32>() / notes.len() as f32
            };

            infos.push(TrackInfo{
                index: infos.len(),
                name: name.clone(),
                channel,
                notes,
                overlap,
                mean_key,
            });
        }
    }

    infos
}

fn pick_melody_track(tracks: &[TrackInfo])->Option<&TrackInfo>{
    tracks
    .iter()
    .filter(|t| t.notes.len()>=8)
    .filter(|t| t.channel!=9)
    .filter(|t| t.mean_key>=45.0 && t.mean_key<=84.0)
    .min_by(|a,b| a.overlap.total_cmp(&b.overlap))
}

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

fn track_instruments(path: &str)->std::collections::HashMap<u8,String>{
    let bytes=std::fs::read(path).expect("could not read midi file");
    let smf=Smf::parse(&bytes).expect("could not parse midi file");
    let mut out: std::collections::HashMap<u8,Vec<u8>>=Default::default();

    for track in &smf.tracks{
        for ev in track{
            if let TrackEventKind::Midi{channel,message: MidiMessage::ProgramChange{program}}=ev.kind{
                let e=out.entry(channel.as_int()).or_default();
                let p=program.as_int();
                if !e.contains(&p){ e.push(p); }
            }
        }
    }

    out.into_iter().map(|(ch,progs)|{
        let names: Vec<String>=progs.iter()
        .map(|&p| format!("{}({p})",GM_FAMILY[(p/8) as usize]))
        .collect();
        (ch,names.join(","))
    }).collect()
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
            instruments.get(&t.channel).map(|s| s.as_str()).unwrap_or("-"),
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

fn selftest(){
    let notes=[60.0f32,62.0,64.0,62.0,60.0,67.0,65.0,64.0];
    let a=intervals(&notes);

    let up: Vec<f32>=notes.iter().map(|n| n+7.0).collect();
    let a_up=intervals(&up);

    let sloppy: Vec<f32>=a.iter().enumerate()
    .map(|(i,&x)| x + if i%2==0{ 0.3 } else { -0.25 })
    .collect();

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

fn save_take(clip: &[f32],sample_rate: u32)->Option<String>{
    if clip.is_empty(){
        return None;
    }

    std::fs::create_dir_all(TAKES_DIR).ok()?;

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

fn synth_track(path: &str,notes: usize,want: Option<usize>)->Option<String>{
    let tracks=load_midi_tracks(path);
    let track=match want{
        Some(i)=>tracks.iter().find(|t| t.index==i)?,
        None=>tracks.iter()
        .filter(|t| t.channel!=9 && t.notes.len()>=notes)
        .filter(|t| t.mean_key>=45.0 && t.mean_key<=84.0)
        .min_by(|a,b| a.overlap.total_cmp(&b.overlap))?,
    };

    let line=melody_line(&track.notes);
    let sr=48000u32;

    let clip: Vec<f32>=line.iter().take(notes).enumerate().flat_map(|(i,&key)|{
        let hz=440.0*2f32.powf((key-69.0)/12.0);
        let n=(sr as f32*0.38) as usize;
        let mut state=(i as u32+1).wrapping_mul(2654435761);

        (0..n).map(move |k|{
            let t=k as f32/sr as f32;
            let env=(t/0.03).min(1.0).min((0.38-t)/0.03).max(0.0);
            state=state.wrapping_mul(1103515245).wrapping_add(12345);
            let noise=(((state>>16) as f32/32768.0)-1.0)*0.01;
            let tp=2.0*std::f32::consts::PI;
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

fn query_shape(query: &[f32],min_useful: usize)->Option<Vec<f32>>{
    let shape=melody_shape(query);

    if shape.len()<4{
        println!("only {} real moves -- hum a longer or less flat phrase",shape.len());
        return None;
    }

    println!("hum shape ({} moves): {}",shape.len(),
        shape.iter().map(|d| format!("{d:+.0}")).collect::<Vec<_>>().join(" "));

    if shape.len()<min_useful{
        println!("WARNING: {} moves is below the {min_useful} needed for a trustworthy result -- a short contour fits almost any song, so treat whatever follows as noise",shape.len());
    }

    Some(shape)
}

fn best_in_file(path: &str,shape: &[f32])->Option<(f32,usize,usize,usize)>{
    let mut best: Option<(f32,usize,usize,usize)>=None;

    for t in &load_midi_tracks(path){
        let refs=melody_shape(&intervals(&melody_line(&t.notes)));
        if refs.len()<shape.len(){ continue; }

        let (cost,start,end)=subsequence_dtw(shape,&refs);
        if best.map_or(true,|(b,_,_,_)| cost<b){
            best=Some((cost,t.index,start,end));
        }
    }

    best
}

fn learn_hum(name: &str,query: &[f32]){
    let Some(shape)=query_shape(query,MIN_USEFUL_HUM_MOVES) else{ return; };

    let line=format!("{name}|{}
",
        shape.iter().map(|d| format!("{d:.0}")).collect::<Vec<_>>().join(" "));

    use std::io::Write;
    match std::fs::OpenOptions::new().create(true).append(true).open(HUMS_FILE){
        Ok(mut f)=>{
            if let Err(e)=f.write_all(line.as_bytes()){
                eprintln!("could not write {HUMS_FILE}: {e}");
                return;
            }
            println!("learned \"{name}\" ({} moves)",shape.len());
            println!("hum it again with `recall` to test, or `learn` it more times to improve it");
        }
        Err(e)=>eprintln!("could not open {HUMS_FILE}: {e}"),
    }
}

fn load_hums()->Vec<(String,Vec<f32>)>{
    std::fs::read_to_string(HUMS_FILE).unwrap_or_default()
    .lines()
    .filter_map(|l|{
        let (name,moves)=l.split_once('|')?;
        let shape: Vec<f32>=moves.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if shape.is_empty(){ None } else { Some((name.to_string(),shape)) }
    })
    .collect()
}

fn recall_hum(query: &[f32]){
    let Some(shape)=query_shape(query,MIN_USEFUL_HUM_MOVES) else{ return; };

    let hums=load_hums();
    if hums.is_empty(){
        println!("nothing learned yet -- `cargo run -- learn <name>` first");
        return;
    }

    let mut best: std::collections::BTreeMap<String,f32>=Default::default();

    for (name,stored) in &hums{
        let (cost,_,_)=if shape.len()<=stored.len(){
            subsequence_dtw(&shape,stored)
        }
        else{
            subsequence_dtw(stored,&shape)
        };

        let e=best.entry(name.clone()).or_insert(f32::INFINITY);
        if cost<*e{ *e=cost; }
    }

    let mut scored: Vec<(f32,String)>=best.into_iter().map(|(n,c)| (c,n)).collect();
    scored.sort_by(|a,b| a.0.total_cmp(&b.0));

    println!("--- {} learned songs, {} recordings ---",scored.len(),hums.len());
    for (cost,name) in &scored{
        println!("  {cost:>7.4}  {name}");
    }

    if scored.len()>=2{
        let margin=scored[1].0 / scored[0].0.max(1e-6);
        println!("best {:.4}, next {:.4} -- {margin:.2}x margin",scored[0].0,scored[1].0);

        if margin<1.5{
            println!("verdict: no clear match");
        }
        else{
            println!("verdict: {}",scored[0].1);
        }
    }
    else{
        println!("only one song learned -- nothing to rank against yet");
    }
}

fn match_against_library(dir: &str,query: &[f32]){
    let Some(shape)=query_shape(query,MIN_USEFUL_MOVES) else{ return; };

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
        if path.extension().and_then(|e| e.to_str())!=Some("mid"){ continue; }

        let name=path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        if let Some((cost,track,start,end))=best_in_file(&path.to_string_lossy(),&shape){
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

fn match_against_midi(path: &str,query: &[f32]){
    let Some(shape)=query_shape(query,MIN_USEFUL_MOVES) else{ return; };

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

const GM_FAMILY: [&str;16]=["Piano","ChromPerc","Organ","Guitar","Bass","Strings",
    "Ensemble","Brass","Reed","Pipe","SynthLead","SynthPad","SynthFX","Ethnic",
    "Percussive","SoundFX"];

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

        let mut query: Vec<f32>=full[full.len()/3..full.len()/3+20].to_vec();
        query[3]+=1.0;
        query[11]-=1.0;
        query.remove(7);

        let mut scored: Vec<(f32,String)>=Vec::new();
        for f in &files{
            if let Some((cost,_,_,_))=best_in_file(&f.to_string_lossy(),&query){
                scored.push((cost,f.file_stem().unwrap().to_string_lossy().to_string()));
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
        Some("selftest")=>selftest(),
        Some("learn")=>{
            match args.get(1){
                Some(name)=>{
                    let query=capture_intervals(args.get(2).map(|s| s.as_str()));
                    learn_hum(name,&query);
                }
                None=>eprintln!("usage: cargo run -- learn <song name> [take.wav]"),
            }
        }
        Some("recall")=>{
            let query=capture_intervals(args.get(1).map(|s| s.as_str()));
            recall_hum(&query);
        }
        Some("verify")=>verify("midi"),
        Some("synth")=>{
            match args.get(1){
                Some(path)=>{
                    let want=args.get(2).and_then(|s| s.parse().ok());
                    synth_track(path,30,want);
                }
                None=>eprintln!("usage: cargo run -- synth <file.mid> [track]"),
            }
        }
        Some("compare")=>compare_takes(),
        Some("match")=>{
            match args.get(1){
                Some(path)=>{
                    let query=capture_intervals(args.get(2).map(|s| s.as_str()));

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
