# Third-party notices

The inherited root LICENSE contains an attribution to Benoit Blanchon and the
PDFium binary distribution. It is retained unchanged. Downloading/redistributing
PDFium requires retaining the applicable upstream binary/source notices. The
helper downloads a shared library for local development; it does not package all
upstream license files for redistribution.

The bundled `brainpipe/assets/cl100k_base.tiktoken` is the cl100k_base vocabulary
used by OpenAI's tiktoken. Its SHA-256 is:

`223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7`

The asset was retrieved from the tiktoken-rs public asset mirror and verified
against the expected hash in OpenAI tiktoken's `openai_public.py`:

- https://github.com/zurawiki/tiktoken-rs/blob/main/tiktoken-rs/assets/cl100k_base.tiktoken
- https://github.com/openai/tiktoken/blob/main/tiktoken_ext/openai_public.py

The encoding definition follows tiktoken's implementation. Its MIT license is
included at `brainpipe/assets/TIKTOKEN_LICENSE.txt`. The `tokens` extra installs
tiktoken; the bundled vocabulary avoids a network fetch during token chunking.

Rust/Python dependencies retain their own licenses. Optional MuPDF, model files
and native runtimes require separate distribution review. This notice is not a
claim that every optional asset is licensed under the same terms as BrainPipe.
