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

## Universal coverage

A handful of MIDI files only recognises a handful of songs. For real coverage,
point it at the Lakh MIDI Dataset's `clean_midi` subset -- about 17,000 files
laid out as `Artist/Title.mid`, which is the part that matters, since a corpus
of hashed filenames can rank a song but never name it.

```
curl -o clean_midi.tar.gz http://hog.ee.columbia.edu/craffel/lmd/clean_midi.tar.gz
tar xzf clean_midi.tar.gz -C corpus/
cargo build --release
./target/release/earworm index corpus/clean_midi index.txt
./target/release/earworm find index.txt
```

Indexing parses every file once and keeps the longest melodic line per song, so
searching is a second rather than minutes. Use the release build: the debug
build is roughly an order of magnitude slower at this size.

Measured on the full corpus, with queries damaged the way a hum is -- two
intervals off by a semitone, one note dropped:

```
12-move query: rank1 50%  top10 70%  mean margin 1.36x
16-move query: rank1 68%  top10 88%  mean margin 1.77x
20-move query: rank1 77%  top10 97%  mean margin 2.47x
26-move query: rank1 86%  top10 97%  mean margin 3.41x
34-move query: rank1 86%  top10 98%  mean margin 3.75x
```

`verify-index` runs that. Query length is the whole story: a dozen moves is a
coin flip against 14,700 songs, while twenty-six lands the right song first
86% of the time and in the top ten 97% of the time. Hum longer.

Scale cuts both ways. Thousands of unrelated melodies each get a chance to
contain a figure like yours, so a margin that would be convincing against ten
songs means nothing against ten thousand. `find` therefore demands 2.0x rather
than the 1.5x used for a small library, and says so when it will not commit.

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
cargo run -- index <dir> [out.txt]     index a whole corpus, recursively
cargo run -- find <index.txt> [wav]    hum, search the indexed corpus
cargo run -- verify-index [index.txt]  retrieval accuracy across the corpus
cargo run -- export [out.json]         melody shapes for the web version
cargo run -- selftest                  matching, against known answers
cargo run -- verify                    every song must identify itself
```

Every recording is saved to `takes/`, numbered. Replaying one gives identical
numbers, which is what makes tuning measurable: with the input frozen, any
change in the output came from the code.

## Web version

The same Rust runs in a browser, which is also how this reaches a phone: a web
page gets microphone access with no install, where native Rust on a phone means
a whole Android or iOS project.

```
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --lib
cp target/wasm32-unknown-unknown/release/earworm.wasm web/
python -m http.server 8731 --directory web
```

No wasm-bindgen and no wasm-pack. The library exports four plain C functions
and the page instantiates the module directly, passing samples through wasm
memory as a Float32Array. That keeps the toolchain to one rustup target.

The corpus works in the browser too. After building `index.txt`:

```
cp index.txt web/corpus.txt
gzip -9 -c index.txt > web/corpus.txt.gz
```

Both, deliberately. 10 MB of shapes compress to 1 MB and the page unpacks them
with `DecompressionStream`, but a browser shield or extension can block a `.gz`
fetch outright -- it surfaces as "Failed to fetch" with no request ever reaching
the server, which is indistinguishable from a server fault until you read the
access log. Plain text always gets through, so the page falls back to it. The corpus is copied into wasm memory once at load, so a
search is a single call rather than fourteen thousand -- about two seconds on a
laptop.

The page does both modes. `cargo run -- export` writes `web/songs.json` --
melody shapes only, no MIDI redistributed -- and the page ranks a hum against
those as well as against hums you have taught it. Taught hums live in
localStorage, so they stay on the device.

Only melody-plausible tracks are matched against, in the browser and on the
desktop alike: not channel 9, under 35% polyphony, in a singable range. A hum
is a melody, so scoring it against basses and pads is fifty-odd chances for
something unrelated to contain a similar figure, and a correct query then loses
to a coincidence. Applying that filter moved both real recorded hums from
mid-table to first, and roughly doubled the regression margins.

A file with no melodic track cannot be identified at all, and drops out of the
tests rather than being scored on its bass line. Web Audio supplies the samples at
whatever rate the device runs at, which is fine because the measured rate is
carried through the pipeline rather than assumed.

It has to be served over http; opening the file directly will not load the wasm,
and microphone access needs a secure context, which localhost counts as.

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

The browser build is checked the same way: tones synthesised in JavaScript,
pushed through the wasm, and the shape compared. A melody transposed up five
semitones with two notes wrong still matches its original at 0.0714 against
0.6429 for an unrelated tune.

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
