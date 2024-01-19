# Binaries

[**Archives of precompiled binaries of reth are available for Windows, macOS and Linux.**](https://github.com/paradigmxyz/reth/releases) They are static executables. Users of platforms not explicitly listed below should download one of these archives.

If you use **macOS Homebrew** or **Linuxbrew**, you can install Reth from Paradigm's homebrew tap:

```text
brew install paradigmxyz/brew/reth
```

If you use **Arch Linux** you can install stable Reth from the AUR using an [AUR helper](https://wiki.archlinux.org/title/AUR_helpers) ([paru][paru] as an example here):

```text
paru -S reth # Stable
paru -S reth-git # Unstable (git)
```

[paru]: https://github.com/Morganamilo/paru

## Signature Verification

You can verify the integrity of a Reth release by checking the signature using GPG.

The release signing key can be fetched from the Ubuntu keyserver using the following command:

```bash
gpg --keyserver keyserver.ubuntu.com --recv-keys A3AE097C89093A124049DF1F5391A3C4100530B4
```

A copy of the key is also included [below](#release-signing-key). Once you have
imported the key you can verify a release signature (`.asc` file) using a
command like this:

```bash
gpg --verify reth-v0.1.0-alpha.14-x86_64-unknown-linux-gnu.tar.gz.asc reth-v0.1.0-alpha.14-x86_64-unknown-linux-gnu.tar.gz
```

Replace the filenames by those corresponding to the downloaded Reth release.

### Release Signing Key

Releases are signed using the key with ID `A3AE097C89093A124049DF1F5391A3C4100530B4`.

```none
-----BEGIN PGP PUBLIC KEY BLOCK-----

mDMEZGLCSBYJKwYBBAHaRw8BAQdA/q69cDkzdUx6EwEoWK7sbm59zsHda7Hgmcq+
7kCg69q0aEdlb3JnaW9zIEtvbnN0YW50b3BvdWxvcyAoVGhpcyBpcyB0aGUga2V5
IHVzZWQgYnkgZ2Frb25zdCB0byBzaWduIFJldGggcmVsZWFzZXMpIDxnZW9yZ2lv
c0BwYXJhZGlnbS54eXo+iJkEExYKAEEWIQSjrgl8iQk6EkBJ3x9TkaPEEAUwtAUC
ZGLCSAIbAwUJAeEzgAULCQgHAgIiAgYVCgkICwIEFgIDAQIeBwIXgAAKCRBTkaPE
EAUwtHvmAQD+w+HZgZkkSqEiQ3XtD8ewRV3rgqFzWsFl+9GGrdmcDAD6AuqcSyAd
yxuMf0tgQDrDLiuXpuWZUsZGvkuzBiiCjwG4OARkYsJIEgorBgEEAZdVAQUBAQdA
tJr3Fle2P/hK+jscCquv5mdptWofGRJwUH3QYLmRlSwDAQgHiH4EGBYKACYWIQSj
rgl8iQk6EkBJ3x9TkaPEEAUwtAUCZGLCSAIbDAUJAeEzgAAKCRBTkaPEEAUwtO77
AP0S+qlwRMbPpsG3No2i2c3Wa5DVqSdHhXpafbRAK9bsCAD+PaytDqwrWJecTyyi
Yg+BMVPJie5ItWPcUCuEYdj/uAM=
=Ao8Q
-----END PGP PUBLIC KEY BLOCK-----
```
