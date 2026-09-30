# Test fixtures

`minisign/`: a **test-only** minisign key and a pre-hashed signature over a dummy
package, regenerated with `python3 make_minisign_fixture.py`. Used by the daemon's
update verifier tests (`services/updates.rs`) and the helper tests. Never trust this key.
