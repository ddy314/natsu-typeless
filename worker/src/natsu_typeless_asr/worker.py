from __future__ import annotations

import json
import os
import struct
import sys
import time
import traceback
from dataclasses import dataclass
from typing import BinaryIO

import numpy as np


PROTOCOL_VERSION = 1
MAX_HEADER_BYTES = 1024 * 1024
MAX_PCM_BYTES = 16_000 * 2 * 300


@dataclass
class Runtime:
    model: object
    processor: object
    torch: object
    device: str
    dtype: object


def read_exact(stream: BinaryIO, size: int) -> bytes:
    chunks: list[bytes] = []
    remaining = size
    while remaining:
        chunk = stream.read(remaining)
        if not chunk:
            raise EOFError("daemon closed the ASR worker pipe")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def read_request(stream: BinaryIO) -> tuple[dict, bytes]:
    header_size = struct.unpack("<I", read_exact(stream, 4))[0]
    if header_size == 0 or header_size > MAX_HEADER_BYTES:
        raise ValueError(f"invalid request header size: {header_size}")
    header = json.loads(read_exact(stream, header_size))
    pcm_size = int(header.get("pcm_bytes", -1))
    if pcm_size < 0 or pcm_size > MAX_PCM_BYTES or pcm_size % 2:
        raise ValueError(f"invalid PCM size: {pcm_size}")
    return header, read_exact(stream, pcm_size)


def write_response(stream: BinaryIO, response: dict) -> None:
    payload = json.dumps(response, ensure_ascii=False, separators=(",", ":")).encode()
    stream.write(struct.pack("<I", len(payload)))
    stream.write(payload)
    stream.flush()


def load_runtime() -> Runtime:
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    import torch
    from transformers import AutoModelForMultimodalLM, AutoProcessor

    model_id = os.environ.get("NATSU_TYPELESS_MODEL", "Qwen/Qwen3-ASR-0.6B-hf")
    revision = os.environ.get(
        "NATSU_TYPELESS_MODEL_REVISION",
        "7f1569a48a89f3e3f4dc3a5c9d28bddd903bc76c",
    )
    device = "cuda:0" if torch.cuda.is_available() else "cpu"
    dtype = torch.bfloat16 if device.startswith("cuda") else torch.float32
    print(f"loading {model_id}@{revision} on {device}", file=sys.stderr, flush=True)
    processor = AutoProcessor.from_pretrained(model_id, revision=revision)
    model = AutoModelForMultimodalLM.from_pretrained(
        model_id,
        revision=revision,
        dtype=dtype,
        device_map=device,
        low_cpu_mem_usage=True,
    ).eval()
    return Runtime(model=model, processor=processor, torch=torch, device=device, dtype=dtype)


def build_transcription_conversation(
    audio: np.ndarray, language_hint: str | None, prompt: str | None
) -> tuple[list[dict], bool]:
    messages: list[dict] = []
    if prompt:
        messages.append(
            {"role": "system", "content": [{"type": "text", "text": prompt}]}
        )
    messages.append(
        {
            "role": "user",
            "content": [{"type": "audio", "audio": audio}],
        }
    )
    if language_hint:
        language_name = {"zh": "Chinese", "en": "English"}.get(
            language_hint.lower(), language_hint
        )
        messages.append(
            {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": f"language {language_name}<asr_text>"}
                ],
            }
        )
        return messages, True
    return messages, False


def canonicalize_vocabulary(values: list[object]) -> list[str]:
    vocabulary: list[str] = []
    for value in values:
        term = str(value).strip()
        if "=>" in term:
            term = term.split("=>", 1)[1].strip()
        elif "->" in term:
            term = term.split("->", 1)[1].strip()
        term = term.removeprefix("@").strip()
        if term and term not in vocabulary:
            vocabulary.append(term)
        if len(vocabulary) >= 128:
            break
    return vocabulary


def transcribe(runtime: Runtime, request: dict, pcm: bytes) -> tuple[str, str, int]:
    if int(request.get("version", 0)) != PROTOCOL_VERSION:
        raise ValueError("unsupported worker protocol version")
    sample_rate = int(request["sample_rate"])
    if sample_rate != 16_000:
        raise ValueError(f"unsupported sample rate: {sample_rate}")
    audio = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768.0
    language = str(request.get("language") or "auto")
    language_hint = None if language == "auto" else language
    vocabulary = canonicalize_vocabulary(request.get("vocabulary", []))
    prompt = f"Vocabulary: {', '.join(vocabulary)}." if vocabulary else None
    conversation, has_language_prefill = build_transcription_conversation(
        audio, language_hint, prompt
    )
    if has_language_prefill:
        inputs = runtime.processor.apply_chat_template(
            conversation,
            tokenize=True,
            return_dict=True,
            continue_final_message=True,
        )
    else:
        inputs = runtime.processor.apply_chat_template(
            conversation,
            tokenize=True,
            return_dict=True,
            add_generation_prompt=True,
        )
    inputs = inputs.to(runtime.device, runtime.dtype)
    started = time.perf_counter()
    with runtime.torch.inference_mode():
        output_ids = runtime.model.generate(
            **inputs,
            max_new_tokens=int(request.get("max_new_tokens", 256)),
            do_sample=False,
        )
    elapsed_ms = round((time.perf_counter() - started) * 1000)
    generated_ids = output_ids[:, inputs["input_ids"].shape[1] :]
    parsed = runtime.processor.decode(generated_ids, return_format="parsed")[0]
    return (
        str(parsed.get("transcription") or "").strip(),
        str(parsed.get("language") or ""),
        elapsed_ms,
    )


def main() -> int:
    output = sys.stdout.buffer
    try:
        runtime = load_runtime()
    except Exception as error:
        write_response(
            output,
            {
                "version": PROTOCOL_VERSION,
                "request_id": "__ready__",
                "ok": False,
                "text": "",
                "language": "",
                "error": f"{type(error).__name__}: {error}",
                "inference_ms": 0,
            },
        )
        traceback.print_exc(file=sys.stderr)
        return 1

    write_response(
        output,
        {
            "version": PROTOCOL_VERSION,
            "request_id": "__ready__",
            "ok": True,
            "text": "",
            "language": "",
            "error": "",
            "inference_ms": 0,
        },
    )

    while True:
        try:
            request, pcm = read_request(sys.stdin.buffer)
        except EOFError:
            return 0
        request_id = str(request.get("request_id", ""))
        try:
            text, language, inference_ms = transcribe(runtime, request, pcm)
            response = {
                "version": PROTOCOL_VERSION,
                "request_id": request_id,
                "ok": bool(text),
                "text": text,
                "language": language,
                "error": "" if text else "no speech recognized",
                "inference_ms": inference_ms,
            }
        except Exception as error:
            traceback.print_exc(file=sys.stderr)
            response = {
                "version": PROTOCOL_VERSION,
                "request_id": request_id,
                "ok": False,
                "text": "",
                "language": "",
                "error": f"{type(error).__name__}: {error}",
                "inference_ms": 0,
            }
        write_response(output, response)


if __name__ == "__main__":
    raise SystemExit(main())
