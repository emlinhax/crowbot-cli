# RE benchmark binaries (M14)

Source for the crackme fixtures the battery's `re` suite reverses. The *source* lives here, never
in the fixture dir — the fixture ships only the stripped binary, so the agent has to reverse it
rather than read the answer off the source (the M11/M12 "answer in reach" lesson).

## vault  (fixture: tests/fixtures/projects/crackme/vault)
Three staged checks (length, a per-index `(s[i]+i) ^ key[i%6]` transform against an embedded
target, a weighted checksum). Accepted passcode: `7Qx!9pL2#aK4` (not present in the binary; only
the key and target arrays are, in .rodata).

Rebuild (x86-64 Linux):
    gcc -O0 -fno-stack-protector -o tests/fixtures/projects/crackme/vault bench/benchmarks/vault.c
    strip tests/fixtures/projects/crackme/vault

## vault2  (fixture: tests/fixtures/projects/crackme/vault2)
Harder: a two-round, chained transform over a 16-byte passcode — round 1 XORs a rolling 5-byte key,
round 2 chains each byte into the previous result, compared to an embedded target; plus a length
gate and a weighted checksum. Accepted passcode: `q7Z-m2L+p9X=r4Kt` (only the key and target arrays
are in the binary, in .rodata).

Rebuild (x86-64 Linux):
    gcc -O0 -fno-stack-protector -o tests/fixtures/projects/crackme/vault2 bench/benchmarks/vault2.c
    strip tests/fixtures/projects/crackme/vault2
