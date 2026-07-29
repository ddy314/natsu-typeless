# Natsu Typeless

Natsu Typeless is a Linux-first voice dictation input method. Hold <kbd>Right Alt</kbd>, speak, release, and the result is committed through the focused fcitx5 input context.

It is deliberately small in scope:

- switchable speech recognition: local `Qwen/Qwen3-ASR-0.6B-hf`, Qwen ASR API,
  or an OpenAI-compatible transcription API;
- optional OpenAI-compatible text cleanup;
- native fcitx5 insertion on Wayland and X11;
- no keyboard simulation, clipboard paste, history database, account system, translation, or “ask AI” mode.

## How it works

```text
Right Alt
    │
    ▼
fcitx5 module ──DBus──▶ Rust daemon ──PipeWire──▶ 16 kHz PCM
    ▲                              │
    │                              ├──▶ ASR provider
    │                              │     ├── Qwen3-ASR worker (local), or
    │                              │     ├── Qwen ASR API, or
    │                              │     └── OpenAI-compatible /audio/transcriptions
    │                              └──▶ OpenAI-compatible text cleanup (optional)
    │
    └──────────── InputContext::commitString(final text)
```

The fcitx5 component is an always-loaded module, not a replacement input method. Pinyin, Rime, and other engines stay selected. Text never goes through `xdotool`, `ydotool`, virtual keyboards, or the clipboard.

See [ARCHITECTURE.md](ARCHITECTURE.md) for lifecycle and protocol details.

## Requirements

- Linux with fcitx5 5.1 or newer
- PipeWire and `pw-record`
- Rust 1.85+, CMake, Ninja, and a C++20 compiler for building
- `uv` and Python 3.12 for the isolated ASR worker
- NVIDIA CUDA is recommended; CPU fallback works but is slower

The initial compatibility target is KDE Plasma on Wayland. Qt, GTK, Chromium/Electron, terminals, native Wayland, and XWayland are covered by the manual test matrix.

## Install for the current user

```bash
./scripts/install-user.sh
```

For local ASR, install the worker:

```bash
natsu-typelessctl setup
```

`setup` creates an isolated environment under
`$XDG_DATA_HOME/natsu-typeless/venv`, installs the pinned inference
dependencies, and downloads the pinned 1.58 GB Qwen model revision.

For a remote ASR API, the local worker is not required. Store its API key
separately:

```bash
natsu-typelessctl asr-key set
```

If cloud text cleanup is enabled, configure its independent key:

```bash
natsu-typelessctl key set
```

Restart fcitx5 from the desktop session, then open `fcitx5-configtool`, select
Addons, and configure **Natsu Typeless**. The addon defaults to:

- hold `Right Alt` to record;
- automatic Chinese/English recognition;
- local ASR (the model loads on first use, not when the daemon starts);
- OpenAI-compatible post-processing enabled;
- API base URL and model configurable without rebuilding;
- surrounding text disabled;
- four-second cloud timeout;
- local ASR model unload after 15 idle minutes.

Run the diagnostics after setup:

```bash
natsu-typelessctl doctor
natsu-typelessctl status
```

## Development

Build and test the Rust side:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

Build the fcitx5 addon:

```bash
cmake -S fcitx5-addon -B fcitx5-addon/build -G Ninja
cmake --build fcitx5-addon/build
```

Set up and test the worker without installing the desktop components:

```bash
uv sync --project worker
uv run --project worker python -m unittest discover -s worker/tests
```

The live prompt regression consumes provider quota and is intentionally ignored:

```bash
cargo test -p natsu-typeless --test prompt_live -- --ignored
```

For a repository-local worker while running the daemon:

```bash
NATSU_TYPELESS_WORKER_PYTHON="$PWD/worker/.venv/bin/python" \
  cargo run -p natsu-typeless --bin natsu-typelessd
```

## Interaction and failure behavior

- The addon refuses to start in password, sensitive, or disabled input fields.
- Existing pinyin/Rime composition must be completed before dictation starts.
- Ordinary keys are held while a dictation session is active; press Escape to cancel.
- Losing focus cancels the session. A stale result is never committed into a different window.
- Text-cleanup failure, timeout, invalid output, or missing credentials falls
  back to the selected ASR provider's raw transcript.
- ASR API failure does not silently load the local model or upload to a second
  provider; fcitx displays a short error.

### ASR providers

Configure recognition under `Addons → Natsu Typeless`:

- `ASR provider`: `local`, `qwen_api`, or `openai_compatible`;
- `ASR API base URL`;
- `ASR API model ID`;
- `Require Bearer API key for ASR`;
- `ASR API timeout in milliseconds`.

`local` launches the isolated Python worker only when dictation first starts.
Switching to either API provider terminates a running local worker, releasing
its CUDA context and VRAM.

For Alibaba Cloud Model Studio Qwen ASR, use:

```text
ASR provider: qwen_api
ASR API base URL: https://dashscope.aliyuncs.com/compatible-mode/v1
ASR API model ID: qwen3-asr-flash
```

Workspace-specific Beijing or Singapore compatible-mode base URLs are also
supported. The daemon wraps the in-memory PCM as a WAV Data URL and calls
`POST {base_url}/chat/completions`. For servers implementing the standard
speech-to-text contract, select `openai_compatible`; the daemon sends a
multipart WAV request to `POST {base_url}/audio/transcriptions`.

The ASR key is stored through Secret Service with
`natsu-typelessctl asr-key set`. `NATSU_TYPELESS_ASR_API_KEY` and
`DASHSCOPE_API_KEY` are supported as environment overrides. Disable the ASR
Bearer-key requirement only for a trusted, unauthenticated local endpoint.

The endpoint can be changed under `Addons → Natsu Typeless` using:

- `OpenAI-compatible API base URL`, for example `https://api.openai.com/v1`
  or `http://127.0.0.1:11434/v1`;
- `OpenAI-compatible model ID`, for example `gpt-4.1-mini` or
  `Qwen/Qwen3-32B`.

The default remains Gemini through its OpenAI-compatible endpoint, preserving
existing installations. `natsu-typelessctl key set` replaces the Bearer API
key. For a local endpoint without authentication, disable `Require Bearer API
key`; the default stays enabled so a missing key never uploads text to a remote
endpoint by accident.

### Domain vocabulary

Recognition vocabulary is data-driven. Installed vocabulary packs live under
`$XDG_DATA_HOME/natsu-typeless/vocabulary.d/`:

- `.txt` files contain one canonical ASR hotword per line;
- explicit user corrections use `heard form => canonical form`;
- `.tsv` entity rules describe a canonical entity, phonetic aliases, positive
  semantic cues, and ordinary-word cues.

The bundled AI pack includes names such as Claude, Opus, Haiku, Codex, and
harness. These are candidate spellings, not global replacements. Entity rules
resolve ambiguous forms from the whole utterance and can reverse an over-eager
hotword when ordinary infrastructure terminology is intended. Editing or
adding vocabulary files does not require rebuilding the project.

## Privacy

- Audio is kept in memory and streamed through pipes; it is not written to disk.
- With local ASR, audio stays on the machine. With an API ASR provider, the
  current recording is sent to the configured endpoint.
- Transcripts and API keys are not logged.
- The configured cloud endpoint receives the current transcript because it performs post-processing.
- Existing text around the cursor is not read or uploaded by default.
- If surrounding context is explicitly enabled, at most 256 characters before and 64 after the cursor are sent.
- Password and sensitive fields always disable capture and context.
- ASR and text-cleanup API keys use separate Secret Service entries. The
  previous Gemini key is read as a text-cleanup migration fallback.
  `apikey.md` is ignored and is never read at runtime.

## License

Natsu Typeless is licensed under the MIT License. Qwen3-ASR model weights are distributed under their own Apache-2.0 license.
