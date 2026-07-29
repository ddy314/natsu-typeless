# Architecture

## Components

### fcitx5 module

The C++ addon is the only component inside the fcitx5 process. It:

- watches key events in the `PreInputMethod` phase;
- keeps one `SessionState` per `InputContext`;
- displays status through `InputPanel::auxUp`;
- communicates with the daemon on the session DBus;
- validates the original context and calls `commitString`.

It never loads ML libraries, opens the microphone, or makes network requests.

### Rust daemon

`natsu-typelessd` owns the single global dictation pipeline:

```text
Idle → Recording → Transcribing → Polishing → ResultReady → Idle
```

Every transition carries a random session ID. Only the DBus unique name that
called `Begin` may call `End`, `Cancel`, or `TakeResult`. Text is returned by a
method response, not broadcast in a signal.

Recording uses the PipeWire `pw-record` client in raw 16 kHz, mono, signed
16-bit mode. The daemon caps the in-memory buffer to the configured recording
duration and interrupts the child cleanly on release.

### ASR provider boundary

The Rust `AsrRouter` selects one provider from runtime settings:

- `local`: isolated Qwen3-ASR Python worker;
- `qwen_api`: Qwen ASR's OpenAI-compatible audio chat-completions contract;
- `openai_compatible`: multipart `POST /audio/transcriptions`.

Recordings remain as PCM in daemon memory. API providers receive a generated
16 kHz mono WAV without a temporary file. Selecting a remote provider stops the
local worker before the new configuration becomes active. Remote failure is
reported as an ASR failure and never triggers an implicit local fallback, so
choosing a low-resource provider cannot unexpectedly allocate VRAM.

### Local ASR worker

PyTorch is isolated in a Python subprocess so CUDA failures do not take down
fcitx5 or the Rust daemon. Requests use this framing:

```text
u32 little-endian JSON length
JSON WorkerRequest
raw PCM bytes (count declared in WorkerRequest)
```

Responses use `u32 length + JSON WorkerResponse`. The model is
`Qwen/Qwen3-ASR-0.6B-hf` pinned to revision
`7f1569a48a89f3e3f4dc3a5c9d28bddd903bc76c`. It loads as BF16 on CUDA and
FP32 on CPU. User and domain vocabulary is passed as a system message in
Qwen3-ASR's chat template and retained for final spelling cleanup. Plain terms
are ASR candidates; `heard => canonical` entries are deterministic correction
rules. Data files under `vocabulary.d` may also provide context-scored entity
aliases, allowing ambiguous words to be resolved in both directions without
hard-coded product combinations. The daemon no longer preloads the model at
startup. It begins loading concurrently with the first recording, and after 15
idle minutes the worker is terminated to release its CUDA context.

### Qwen ASR API

The Qwen adapter wraps raw PCM in a WAV Data URL and sends it as
`input_audio` to `POST {base_url}/chat/completions`. Recognition language is
passed through `asr_options` when explicitly selected; vocabulary is supplied
as recognition context. The non-streaming response text and detected-language
annotation are normalized into the same `TranscriptResult` returned by the
local worker.

### OpenAI-compatible transcription API

The generic adapter posts an in-memory `recording.wav` multipart field to
`POST {base_url}/audio/transcriptions`, along with `model`, optional `language`,
and vocabulary as `prompt`. Both API adapters share bounded timeouts, optional
Bearer authentication, and the same downstream vocabulary/entity cleanup.

### OpenAI-compatible post-processing

The active prompt is the versioned resource
`daemon/prompts/dictation-v2.txt`. The transcript and optional bounded context
are JSON data, never interpolated into instructions. The prompt requires active
editing (not punctuation-only cleanup), demonstrates filler removal,
self-correction and list formatting, and still forbids answering or executing
the dictated content.

The provider boundary uses `POST {base_url}/chat/completions` with standard
`model`, `messages`, bounded `max_tokens`, deterministic `temperature: 0`,
Bearer
authorization, and non-streaming response fields.
Both the base URL and model ID are runtime fcitx settings. HTTPS services and
HTTP services such as Ollama or vLLM are supported. Bearer authentication is
required by default and may be explicitly disabled for an unauthenticated
local service. Gemini's OpenAI-compatible endpoint remains the default for
migration compatibility.

An interactive request is attempted once with a four-second total timeout.
Empty results or output longer than
`max(raw + 128 characters, raw × 1.75)` are rejected. Any failure commits the
local transcript.

## DBus contract

The canonical interface is
[`data/io.github.ddy314.NatsuTypeless.xml`](data/io.github.ddy314.NatsuTypeless.xml).

- `Configure(config_json)` applies bounded ASR-provider and post-processing
  runtime settings. It never accepts API keys.
- `Begin(session_id, options_json)` starts capture and records the caller.
- `End(session_id)` stops capture and schedules processing.
- `Cancel(session_id)` discards audio/result state.
- `GetStatus()` returns operational state without transcript content.
- `TakeResult(session_id)` returns and deletes the completed result.
- `StateChanged` and `ResultReady` signals never contain dictated text.

Results expire after 30 seconds. The daemon accepts one active microphone
session because the machine has one default dictation source.

## Deliberate exclusions

There is no history, SQLite database, application classifier, scene router,
selection editor, translation mode, chat mode, tray dashboard, synthetic
keyboard injection, clipboard fallback, or automatic failover between audio
providers.
