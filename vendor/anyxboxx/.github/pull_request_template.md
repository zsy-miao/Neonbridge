## Change

<!-- Please write in English. The observable behaviour that changes, and why. One reviewable thing per PR. -->

Linked issue: Fixes # / Refs #

## Verification

<!-- For each check: the exact command and its result (copy the last line).
     GPU: card or WARP, and the first difference if there is one.
     If you could not run a check, say so and name what was missing. -->

```
python -m pytest                → … passed
python -m ruff check --select W605,F,E9 PrototypeScarlett tests tools → All checks passed!
```

## Oracle and test data

<!-- Decoder: compared with the LLVM tables (`tools/rdna2_mnemonics.py … --verifier`).
     HLSL translation: compared with the reference interpreter, byte for byte.
     New test data: where it comes from (built in the tests, LLVM…) and its licence.
     Failure case: which test fails without this change? -->

## Checklist

- [ ] The PR covers a single reviewable behaviour.
- [ ] A test fails without this change and passes with it.
- [ ] The commands and results of each check are given above.
- [ ] An unimplemented function returns an error (`E_NOTIMPL`), never a fake success.
- [ ] README: the same change in all 9 languages (`test_readme_sync.py` checks it).
- [ ] No game, console or SDK file, no key, no dump, no download link.
- [ ] Findings drawn from a title: limited to what rule 2 of `AGENTS.md` allows (names, slots, argument shapes, flag values, HRESULTs, IIDs, counts), with how they were obtained; no address or RVA.
- [ ] Code taken from another project: compatible licence and credit in `THIRD_PARTY.md`.

<!-- If this change was made with AI assistance, say so here. -->
