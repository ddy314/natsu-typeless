import io
import json
import struct
import unittest

import numpy as np

from natsu_typeless_asr.worker import (
    build_transcription_conversation,
    canonicalize_vocabulary,
    read_request,
    write_response,
)


class ProtocolTests(unittest.TestCase):
    def test_round_trip_framing(self):
        header = {"pcm_bytes": 4, "request_id": "abc"}
        encoded = json.dumps(header).encode()
        stream = io.BytesIO(
            struct.pack("<I", len(encoded)) + encoded + b"\x00\x00\x01\x00"
        )
        actual, pcm = read_request(stream)
        self.assertEqual(actual, header)
        self.assertEqual(pcm, b"\x00\x00\x01\x00")

        output = io.BytesIO()
        write_response(output, {"ok": True, "text": "你好"})
        raw = output.getvalue()
        size = struct.unpack("<I", raw[:4])[0]
        self.assertEqual(json.loads(raw[4 : 4 + size])["text"], "你好")

    def test_rejects_odd_pcm_length(self):
        header = {"pcm_bytes": 3}
        encoded = json.dumps(header).encode()
        stream = io.BytesIO(struct.pack("<I", len(encoded)) + encoded + b"abc")
        with self.assertRaises(ValueError):
            read_request(stream)

    def test_hotwords_are_a_system_message_not_processor_kwargs(self):
        audio = np.zeros(160, dtype=np.float32)
        conversation, has_prefill = build_transcription_conversation(
            audio, "zh", "Vocabulary: AcmeAI, ExampleTerm."
        )
        self.assertTrue(has_prefill)
        self.assertEqual(conversation[0]["role"], "system")
        self.assertEqual(
            conversation[0]["content"][0]["text"],
            "Vocabulary: AcmeAI, ExampleTerm.",
        )
        self.assertEqual(conversation[1]["content"][0]["type"], "audio")
        self.assertIs(conversation[1]["content"][0]["audio"], audio)
        self.assertEqual(
            conversation[2]["content"][0]["text"], "language Chinese<asr_text>"
        )

    def test_auto_language_uses_generation_prompt(self):
        conversation, has_prefill = build_transcription_conversation(
            np.zeros(160, dtype=np.float32), None, None
        )
        self.assertFalse(has_prefill)
        self.assertEqual([message["role"] for message in conversation], ["user"])

    def test_vocabulary_markers_and_corrections_are_asr_canonicalized(self):
        self.assertEqual(
            canonicalize_vocabulary(
                ["@Claude", "MesoS => Mythos", "harness", "harness"]
            ),
            ["Claude", "Mythos", "harness"],
        )


if __name__ == "__main__":
    unittest.main()
