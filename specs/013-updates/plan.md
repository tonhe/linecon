# Implementation Plan: Updates From GitHub Releases

CI signs a small text manifest of file hashes with an Ed25519 key held only in a repository secret, and the app checks it with `ed25519-dalek`. The HTTP client uses the OS certificate store and proxy settings, because client networks inspect TLS. TLS is not what protects updates; the signature is.
