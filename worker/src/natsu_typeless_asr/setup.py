from __future__ import annotations

import os

from huggingface_hub import snapshot_download


def main() -> None:
    model = os.environ.get("NATSU_TYPELESS_MODEL", "Qwen/Qwen3-ASR-0.6B-hf")
    revision = os.environ.get(
        "NATSU_TYPELESS_MODEL_REVISION",
        "7f1569a48a89f3e3f4dc3a5c9d28bddd903bc76c",
    )
    path = snapshot_download(repo_id=model, revision=revision)
    print(f"Model installed: {path}")


if __name__ == "__main__":
    main()

