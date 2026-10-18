# challenge binaries

Reverse-engineering challenges. Each takes one passcode on the command line and prints
"Access granted. Vault unlocked." for the right one, "Access denied." otherwise.

- `./vault`  — recover its passcode by reversing it.
- `./vault2` — harder: a two-round, chained check.

The passcodes are not stored in the binaries; recover them by reversing.
