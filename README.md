# Natsu Typeless

Natsu Typeless is a Linux-first voice dictation input method. Hold <kbd>Right Alt</kbd>, speak, release, and the result is committed through the focused fcitx5 input context.

It is deliberately small in scope:

- local speech recognition with `Qwen/Qwen3-ASR-0.6B-hf`;
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
    │                              ├──▶ Qwen3-ASR worker (local)
    │                              └──▶ OpenAI-compatible API (optional)
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
natsu-typelessctl setup
natsu-typelessctl key set
```

`setup` creates an isolated environment under
`$XDG_DATA_HOME/natsu-typeless/venv`, installs the pinned inference
dependencies, and downloads the pinned 1.58 GB Qwen model revision.

Restart fcitx5 from the desktop session, then open `fcitx5-configtool`, select
Addons, and configure **Natsu Typeless**. The addon defaults to:

- hold `Right Alt` to record;
- automatic Chinese/English recognition;
- OpenAI-compatible post-processing enabled;
- API base URL and model configurable without rebuilding;
- surrounding text disabled;
- four-second cloud timeout;
- ASR model unload after 15 idle minutes.

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
- Cloud failure, timeout, invalid output, or missing credentials falls back to the local Qwen transcript.
- ASR failure does not fabricate or paste text; fcitx displays a short error.

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
- Transcripts and API keys are not logged.
- The configured cloud endpoint receives the current transcript because it performs post-processing.
- Existing text around the cursor is not read or uploaded by default.
- If surrounding context is explicitly enabled, at most 256 characters before and 64 after the cursor are sent.
- Password and sensitive fields always disable capture and context.
- The cloud API key is stored through Secret Service. The previous Gemini key is read as a migration fallback. `apikey.md` is ignored and is never read at runtime.

## License

Natsu Typeless is licensed under the MIT License. Qwen3-ASR model weights are distributed under their own Apache-2.0 license.
