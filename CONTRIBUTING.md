# Contributing

Natsu Typeless intentionally focuses on reliable voice insertion through
fcitx5. Please discuss substantial scope changes before opening a pull request.

Before submitting changes, run:

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cmake -S fcitx5-addon -B fcitx5-addon/build -G Ninja
cmake --build fcitx5-addon/build
uv run --project worker python -m unittest discover -s worker/tests
```

Changes to the post-processing prompt must add or update cases in
`daemon/tests/prompt_cases.json`. Do not add real transcripts, recordings,
credentials, model weights, or generated virtual environments to the
repository.

Manual UI changes must be checked on at least one Wayland Qt application, one
GTK application, a Chromium/Electron text field, and a terminal. Include the
observed release-to-commit timing in the pull request.
