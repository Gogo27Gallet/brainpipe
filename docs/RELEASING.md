# Release gates

Publishing is a separate maintainer action. The repository's build workflow runs for `v*` tags or manual dispatch and can
produce artifacts; creating/updating a PR does not publish a package.

1. Confirm BrainPipe's own license and contributor rights. The inherited root
   LICENSE appears to come from the PDFium binary package. Retain its attribution;
   do not treat it as proof of licensing for every file in this repository.
2. Review notices for PDFium, optional MuPDF/ONNX assets, OCR models and bundled
   tokenizer data. Include the licenses/notices of any binaries actually shipped.
3. Bump Python/Cargo versions together; bump cache/schema versions when behavior
   or serialized compatibility requires it. Update CHANGELOG.md.
4. Pass native and isolated regression tests on Linux/macOS/Windows and supported
   Python versions. Treat configured CI matrices as pending until runs pass.
5. Run synthetic evaluation and mutation smoke tests. Evaluate OCR/complex tables
   on a labeled representative corpus before claiming their accuracy.
6. Build wheels and an sdist, install them into clean environments, check the CLI,
   API demo assets, bundled tokenizer and optional imports. Test the pinned pypdfium2 dependency in a clean wheel installation outside the
   checkout, with no manually downloaded library and no runtime network access.
7. Record release-build performance with corpus hashes, configuration and text
   equivalence checks. Never publish a generic speedup claim from a tiny fixture.
8. Test job overload, cancellation, retention, worker crashes and resource limits
   under the intended deployment OS. Document unavailable OS limits on Windows.
9. Review the release notes, artifact contents and dependency advisories, then
   create a version tag/publish through the maintainer's approved release flow.
