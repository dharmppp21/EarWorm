# Earworm

Hum a tune, get the song name. Written in Rust, with the signal processing
implemented from scratch rather than pulled from a library.

Shazam cannot do this. It fingerprints the exact frequency peaks of a specific
recording, and humming destroys every one of them. Recognising a hum is the
opposite problem, and needs the opposite technique.

## How it works

```
microphone -> pitch per frame -> notes -> intervals -> shape -> match -> song
```

**Pitch detection (YIN).** A 4096-sample window slides in 1024-sample hops. For
each window, the difference function is computed at every lag in the 80-1400 Hz
range, cumulative-mean-normalised, and searched for the first dip below a
threshold -- then walked downhill to the true local minimum. That last step
matters: stopping at the threshold crossing lands on the falling edge of the
valley rather than its bottom, which reads every note about 10% sharp.

The normalised difference value doubles as a confidence score, and it is really
a signal-to-noise readout: it is scale-invariant, so amplifying a quiet
recording does not improve it. If confidences will not drop, the fix is at the
microphone.

**Cleaning.** Frames above a confidence cutoff are dropped as unvoiced. The rest
convert to semitones via `12 * log2(hz / 440) + 69`, putting them on the MIDI
numbering so a hum and a MIDI file share a scale. A 5-frame median filter
removes single-frame octave slips -- median rather than mean, because a median
only cares about rank, so one wrong value among five cannot be the middle one no
matter how far off it is.

**Notes.** Frames group into notes, ending when the pitch leaves the note's
running average by more than a semitone or the voice stops for longer than a
dropped frame. Notes landing about 12 semitones off their neighbours and
immediately returning are folded back -- that is the pitch detector locking onto
half the true period.

**Intervals.** A melody is the gaps between notes, not the notes. Sing it in any
key and every note changes while every gap stays identical. Repeated notes are
then dropped and the rest rounded to whole semitones: what identifies a song is
where the melody moves, and a hum spells a repeat as `-0.6` where a MIDI spells
it as exactly `0`.

**Matching (subsequence DTW).** A short hum has to match a section anywhere
inside a long song, so the first row is zero-filled (starting anywhere is free)
and the best cell of the last row is taken (ending anywhere is free). Warping
absorbs notes you missed or added. Every track of every song is scored, and the
margin between best and runner-up decides whether anything was recognised --
DTW always returns a number, so a bunched field means no match whatever sits on
top.

## Setup

```
cargo build --release
```

Reference songs are MIDI files placed in `midi/`. They are not included --
they are commercial recordings, not mine to redistribute. Any `.mid` works;
drop them in and they are picked up automatically.

Not every MIDI is usable. Many are karaoke backing arrangements with the vocal
deliberately stripped out, and no hum can ever match those. Check first:

```
cargo run -- midi/song.mid
```

That prints every track with its instrument and how polyphonic it is. A melody
track is nearly monophonic and sits in a singable range. A file showing only
Bass, Guitars, Pads and Drums has no tune in it.

## Usage

```
cargo run                              hum, print the melody
cargo run -- learn <name> [take.wav]   store a hum as a reference
cargo run -- recall [take.wav]         hum, match against stored hums
cargo run -- match midi                hum, rank every song in the library
cargo run -- match midi <take.wav>     rerun a saved take instead of humming
cargo run -- match <file.mid>          rank the tracks within one song
cargo run -- <file.mid>                inspect tracks and instruments
cargo run -- synth <file.mid> [track]  render a melody to audio
cargo run -- compare                   hum twice, measure agreement
cargo run -- selftest                  matching, against known answers
cargo run -- verify                    every song must identify itself
```

Every recording is saved to `takes/`, numbered. Replaying one gives identical
numbers, which is what makes tuning measurable: with the input frozen, any
change in the output came from the code.

## Testing

Matching fails quietly. A wrong implementation still returns a confident number
and ranks something first, so the tests check properties with known answers
rather than eyeballing output.

`selftest` covers the algorithm. Transposing a melody up a fifth must cost
exactly zero, since intervals are differences -- that is the assumption the
whole design rests on. A missed note, an added note and slightly-off singing
score 0.14-0.27 against 5.5 for an unrelated tune.

`verify` covers the library. Each song's own melody is damaged the way a hum is
damaged -- two intervals off by a semitone, one note dropped -- and the library
must still rank that song first. An undamaged copy would only prove the file
loader works.

`synth` covers the whole chain. It renders a MIDI melody as tones, and running
`match` on that audio exercises everything except the microphone. Fed a real
melody it reproduces that melody's shape exactly and identifies the song at cost
`0.0000`.

## What works, and what does not

The recogniser is correct, end to end, and the tests above demonstrate it. Two
independently sourced arrangements of one song, in different keys, rank first
and second against each other -- transposition invariance confirmed on real
files rather than synthetic ones.

Matching a hum against a MIDI melody is the hard version, and it does not work
well. Real hums flatten intervals, merge repeated notes and drop notes, and
against an exact melody all of that error sits on one side. Real attempts here
never cleared a 1.13x margin.

Matching a hum against other hums works, which is what `learn` and `recall` do
and what commercial systems have always done. Both sides carry the same
distortions, so they cancel. Measured: two recordings of a phrase stored as a
reference, then a third recording of it -- never stored -- identified at cost
0.0769 with a 5.20x margin, on six moves. MIDI matching needs twice that many
moves and still fails.

So the recogniser has two modes with very different characters. Against a MIDI
library it can name a song it has never heard hummed, but only from an accurate
query. Against learned hums it is reliable, but only for songs it has been
taught. The second is the one that actually works today.
