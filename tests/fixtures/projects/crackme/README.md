# vault

A reverse-engineering challenge binary. `./vault <passcode>` accepts exactly one passcode
and prints "Access granted. Vault unlocked."; everything else is denied.

The passcode is not stored in the binary — recover it by reversing `./vault`.
