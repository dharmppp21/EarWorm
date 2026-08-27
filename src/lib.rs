pub const WINDOW_SIZE: usize=4096;

pub const HOP_SIZE: usize=1024;

pub const MIN_FREQ_HZ: f32=80.0;

pub const MAX_FREQ_HZ: f32=1400.0;

pub const YIN_THRESHOLD: f32=0.25;

pub const VOICED_MAX_CMNDF: f32=0.30;

pub const NOTE_NAMES: [&str;12]=["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"];

pub const MEDIAN_WINDOW: usize=5;

pub const NOTE_CHANGE_ST: f32=1.0;

pub const MIN_NOTE_FRAMES: usize=5;

pub const MAX_GAP_FRAMES: usize=2;

pub const OCTAVE_JUMP_ST: f32=6.0;

pub const MIN_MOVE_ST: f32=0.75;

pub const MIN_USEFUL_MOVES: usize=12;

pub const MIN_USEFUL_HUM_MOVES: usize=5;

pub fn track_pitch(samples: &[f32],sample_rate:u32)->Vec<(f32,f32)>{
    let mut pitches=Vec::new();
    let mut start=0;
    while start+WINDOW_SIZE<=samples.len(){
        let frame= &samples[start..start + WINDOW_SIZE];

        pitches.push(detect_pitch_yin(frame,sample_rate));
        start+=HOP_SIZE;
    }
    pitches
}

pub fn difference_at_lag(frame: &[f32], lag: usize)->f32{
    let n=frame.len()-lag;
    let mut sum=0.0;
    for i in 0..n{
        let diff=frame[i]-frame[i+lag];
        sum+=diff*diff;
    }
    sum
}

pub fn detect_pitch_yin(frame: &[f32], sample_rate: u32)->(f32,f32){
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

pub fn hz_to_semitones(hz: f32)->f32{
    12.0*(hz/440.0).log2()+69.0
}

pub fn clean_pitch_track(pitches: &[(f32,f32)])->Vec<Option<f32>>{
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

pub fn note_name(semitones: f32)->String{
    let midi=semitones.round() as i32;
    let name =NOTE_NAMES[midi.rem_euclid(12) as usize];
    let octave=midi.div_euclid(12)-1;
    format!("{name}{octave}")
}

pub fn median_filter(track: &[Option<f32>])->Vec<Option<f32>>{
    (0..track.len())
    .map(|i|{
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

pub struct Note{
    pub semitones: f32,
    pub start: usize,
    pub end: usize,
}

pub fn push_note(notes: &mut Vec<Note>,current: &mut Vec<f32>,start: usize,end: usize){
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

pub fn segment_notes(track: &[Option<f32>])->Vec<Note>{
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

    push_note(&mut notes,&mut current,start,end);
    notes
}

pub fn fix_octaves(notes: &mut [Note])->usize{
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

pub fn intervals(pitches: &[f32])->Vec<f32>{
    pitches
    .windows(2)
    .map(|pair| pair[1]-pair[0])
    .collect()
}

pub fn dtw(a: &[f32],b: &[f32])->f32{
    if a.is_empty() || b.is_empty(){
        return f32::INFINITY;
    }

    let n=a.len();
    let m=b.len();

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

    prev[m] / (n+m) as f32
}

pub fn subsequence_dtw(query: &[f32],reference: &[f32])->(f32,usize,usize){
    if query.is_empty() || reference.len()<query.len(){
        return (f32::INFINITY,0,0);
    }

    let n=query.len();
    let m=reference.len();

    let mut prev=vec![0.0f32; m+1];
    let mut curr=vec![f32::INFINITY; m+1];

    let mut prev_start: Vec<usize>=(0..=m).collect();
    let mut curr_start=vec![0usize; m+1];

    for i in 1..=n{
        curr[0]=f32::INFINITY;
        curr_start[0]=0;

        for j in 1..=m{
            let cost=(query[i-1]-reference[j-1]).abs();

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

pub fn melody_shape(steps: &[f32])->Vec<f32>{
    steps
    .iter()
    .copied()
    .filter(|d| d.abs()>=MIN_MOVE_ST)
    .map(|d| d.round())
    .collect()
}

// --- browser entry points -------------------------------------------------
// Called straight from JS with no bindgen: allocate a buffer, copy samples in,
// call analyse, read the shape back out of the same memory.

static mut SHAPE: [f32;512]=[0.0;512];

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize)->*mut f32{
    let mut v=Vec::<f32>::with_capacity(len);
    let p=v.as_mut_ptr();
    std::mem::forget(v);
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn shape_ptr()->*const f32{
    &raw const SHAPE as *const f32
}

/// Samples in, number of moves out. The moves themselves land in SHAPE.
#[unsafe(no_mangle)]
pub extern "C" fn analyse(ptr: *const f32,len: usize,sample_rate: u32)->usize{
    let clip=unsafe{ std::slice::from_raw_parts(ptr,len) };

    let track=median_filter(&clean_pitch_track(&track_pitch(clip,sample_rate)));
    let mut notes=segment_notes(&track);
    fix_octaves(&mut notes);

    let pitches: Vec<f32>=notes.iter().map(|n| n.semitones).collect();
    let shape=melody_shape(&intervals(&pitches));
    let n=shape.len().min(512);

    unsafe{
        for i in 0..n{ SHAPE[i]=shape[i]; }
    }

    n
}

/// Compare two shapes already sitting in wasm memory. Lower is better.
#[unsafe(no_mangle)]
pub extern "C" fn compare(a: *const f32,alen: usize,b: *const f32,blen: usize)->f32{
    let x=unsafe{ std::slice::from_raw_parts(a,alen) };
    let y=unsafe{ std::slice::from_raw_parts(b,blen) };

    if x.len()<=y.len(){ subsequence_dtw(x,y).0 } else { subsequence_dtw(y,x).0 }
}

static mut CORPUS: Vec<Vec<f32>>=Vec::new();
static mut RESULTS: [f32;20]=[0.0;20];

#[unsafe(no_mangle)]
pub extern "C" fn corpus_clear(){
    unsafe{ (*&raw mut CORPUS).clear(); }
}

/// Copy one song's shape in. Keeping the corpus inside wasm means a search is
/// one call instead of fourteen thousand, and nothing is allocated per query.
#[unsafe(no_mangle)]
pub extern "C" fn corpus_add(ptr: *const f32,len: usize)->usize{
    let s=unsafe{ std::slice::from_raw_parts(ptr,len) }.to_vec();
    unsafe{
        let c=&mut *&raw mut CORPUS;
        c.push(s);
        c.len()-1
    }
}

/// Score the query against everything stored. Writes the best ten as
/// (index, cost) pairs into RESULTS and returns how many were written.
#[unsafe(no_mangle)]
pub extern "C" fn search(ptr: *const f32,len: usize)->usize{
    let q=unsafe{ std::slice::from_raw_parts(ptr,len) };
    let corpus=unsafe{ &*&raw const CORPUS };

    let mut scored: Vec<(f32,usize)>=corpus.iter().enumerate()
    .filter(|(_,s)| s.len()>=q.len())
    .map(|(i,s)| (subsequence_dtw(q,s).0,i))
    .collect();

    scored.sort_by(|a,b| a.0.total_cmp(&b.0));

    let n=scored.len().min(10);
    unsafe{
        let r=&mut *&raw mut RESULTS;
        for k in 0..n{
            r[k*2]=scored[k].1 as f32;
            r[k*2+1]=scored[k].0;
        }
    }

    n
}

#[unsafe(no_mangle)]
pub extern "C" fn results_ptr()->*const f32{
    &raw const RESULTS as *const f32
}
