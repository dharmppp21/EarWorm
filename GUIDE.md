# Earworm — hum a tune, get the song

You're building this one yourself. This is the map, not the code.

---

## Read this first — it changes the whole project

**Shazam's algorithm cannot do this.** Shazam fingerprints the *exact recording*:
it locks onto specific frequency peaks produced by particular instruments in a
particular mix. Hum that same song and every one of those peaks is gone. Shazam
matches audio; it does not match music.

So you're not building Shazam. You're building **Query by Humming** — a different,
older, and honestly more interesting problem. The insight:

> A melody is not a sequence of pitches. It's a sequence of *pitch changes*.

If you hum "Happy Birthday" in any key, at any speed, in any voice, the pattern
`same, up 2, down 2, up 5, down 1` is identical every time. That relative pattern
is the fingerprint. Key-invariant and singer-invariant, for free.

Handling the "any speed" part is what DTW is for, in stage 5.

---

## The pipeline

```
your hum  →  pitch per frame  →  clean note sequence  →  interval contour
                                                              ↓
                                                        DTW match
                                                              ↑
MIDI files  →  melody track  →  note sequence  →  interval contour
```

---

## Stack

```
Python 3.11
numpy          array maths + FFT
sounddevice    microphone capture
mido           MIDI file parsing
matplotlib     looking at your own data (you will need this constantly)
```

Later, to ship it: `fastapi` + a small HTML page using MediaRecorder.

**Write yourself** (this is the learning): pitch detection, note segmentation,
contour encoding, DTW, ranking.
**Don't write yourself** (not the lesson, and you'd do it worse): FFT, MIDI parsing,
audio I/O.

---

## Stage 1 — Hear yourself (half a day)

Capture mono audio from the mic at 22050 Hz in chunks. Compute an STFT with
`numpy.fft.rfft` over Hann-windowed frames (2048 samples, hop 512). Plot the
spectrogram.

**Learn:** why windowing exists, what the hop size trades off, why a 2048-sample
window at 22 kHz gives ~10 Hz resolution — and why that's already a problem for
low notes.

**Verify:** hum a slow rising "ooooh". You should see a bright line climb, with
faint parallel lines above it. Those are harmonics, and they're about to cause
you real trouble.

---

## Stage 2 — Find the pitch (1.5 days, the hard one)

Write an autocorrelation pitch detector: correlate a frame with time-shifted
copies of itself, and the lag with the strongest correlation is your period.
Pitch = sample_rate / lag.

Then improve it to **YIN**, which is autocorrelation done properly: use the
cumulative mean normalised difference function and take the first dip below a
threshold rather than the global maximum.

**The gotcha that will eat your afternoon — octave errors.** Naive autocorrelation
confidently reports half or double the true pitch, because a wave that repeats
every N samples also repeats every 2N. YIN's "first dip, not best dip" rule exists
precisely to fix this. Expect to fight it anyway.

**Why not just take the loudest FFT bin?** Try it and see. For a hummed note the
loudest partial is often a harmonic, not the fundamental — and voices sometimes
have a *missing fundamental* where the loudest bin isn't the pitch you hear at all.

**Verify:** open any tuner app, hum a steady A. Your detector should read
440 Hz ± 5. Then hum a low note and watch it break. Fix that.

---

## Stage 3 — Clean it up (half a day)

Raw pitch tracks are garbage — silence produces noise, breaths produce nonsense,
vibrato wobbles ±50 cents.

- Drop frames below an energy threshold (silence) or below a YIN confidence cut.
- Median filter across ~5 frames to kill single-frame spikes.
- Convert Hz to semitones: `12 * log2(f / 440) + 69`. Now you're in a **linear**
  space where "up a fifth" is +7 everywhere, instead of a multiplication.

**Verify:** sing a scale. Plot it. You should see a clean staircase, not a hairball.

---

## Stage 4 — Notes, then contour (1 day)

Group consecutive frames of similar pitch into **notes**: a new note starts on a
sustained jump greater than ~0.8 semitones, or after a gap of silence. Keep each
note's median pitch and duration.

Then throw the absolute pitches away and keep only the differences:

```
notes:     69, 69, 71, 69, 74, 73
intervals:  0, +2, -2, +5, -1
```

That's your fingerprint. Key-invariant, because transposing the whole hum shifts
every note and changes no interval.

**Verify:** hum the same tune twice, once low and once high. The interval sequences
should be nearly identical. If they're not, go back to stage 2 — this is the test
that tells you your pitch detector is actually working.

---

## Stage 5 — Build the song database (1 day)

You need melodies to match against. **MIDI files are the answer** — they're already
symbolic notes, so no audio analysis needed, and large free collections exist
(the Lakh MIDI Dataset is the standard one; start with a few hundred files you
recognise, not all 176,000).

For each file: parse with `mido`, pick the melody track (heuristics — the track
with the most notes in vocal range, or the highest-pitched monophonic line), take
note-on events in order, and run them through the *exact same* interval encoding
from stage 4.

**Critical:** the hum path and the MIDI path must produce identical representations.
Any asymmetry here and nothing will ever match. Factor out one shared function
and call it from both sides.

Store as JSON or SQLite: song title, interval array, note durations.

---

## Stage 6 — Match with DTW (1 day)

Your hum is 8 notes; the song is 200. You started in the middle. You hummed it
slower than the record. Plain sequence comparison is hopeless.

**Dynamic Time Warping** solves exactly this: it finds the best alignment between
two sequences that may run at different speeds, by filling a cost matrix where
each cell holds "cheapest way to align the first i of one with the first j of the
other." It's the same dynamic-programming shape as edit distance, and it's about
15 lines.

Two things you must get right:
- **Subsequence DTW**, not full DTW — the hum matches *part* of the song, so the
  alignment is free to start and end anywhere in the reference.
- **Normalise by path length**, or short songs win everything.

Rank all songs by cost, return the top 10.

**Verify:** hum something in your database. It should be top 3. If it's top 50,
plot the aligned contours side by side and look at where they diverge — the bug is
almost always in stage 2 or in an asymmetry between your two encoders.

---

## Stage 7 — Make it real (1 day)

FastAPI endpoint that takes a recorded blob, runs the pipeline, returns ranked
matches. A single HTML page with a record button using MediaRecorder. Show the
detected note contour back to the user — seeing your own hum as notes is half
the delight, and it makes failures explainable instead of mysterious.

---

## Total: ~7 days

## What will actually go wrong

1. **Octave errors** in pitch detection. Budget real time for this.
2. **Your MIDI melody extraction picks the bass line.** Listen to what you
   extracted before blaming the matcher.
3. **You hum worse than you think.** Test with a hum you've verified is roughly
   in tune, or you'll debug working code.
4. **Too many songs, too early.** Start with 50 you know well. Scale after it works.

## Where this loses to Google's version, honestly

Google trains a neural network to map hums and recordings into a shared embedding
space, learned from real humming data. It's more accurate, and it works on
recordings directly instead of needing MIDI.

Your contour + DTW approach is the classical MIR method — it's explainable, it
needs no training data, it runs on a laptop, and you'll understand every line of
it. That's a better answer in an interview than "I fine-tuned a model."

Note the ceiling out loud, and know what you'd do next.
