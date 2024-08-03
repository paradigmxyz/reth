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
gpg --keyserver keyserver.ubuntu.com --recv-keys 50FB7CC55B2E8AFA59FE03B7AA5ED56A7FBF253E
```

A copy of the key is also included [below](#release-signing-key). Once you have
imported the key you can verify a release signature (`.asc` file) using a
command like this:

```bash
gpg --verify reth-v0.2.0-beta.9-x86_64-unknown-linux-gnu.tar.gz.asc reth-v0.1.0-beta.9-x86_64-unknown-linux-gnu.tar.gz
```

Replace the filenames by those corresponding to the downloaded Reth release.

### Release Signing Key

Releases are signed using the key with ID [`50FB7CC55B2E8AFA59FE03B7AA5ED56A7FBF253E`](https://keyserver.ubuntu.com/pks/lookup?search=50FB7CC55B2E8AFA59FE03B7AA5ED56A7FBF253E&fingerprint=on&op=index).

```none
-----BEGIN PGP PUBLIC KEY BLOCK-----

mDMEZl4GjhYJKwYBBAHaRw8BAQdAU5gnINBAfIgF9S9GzZ1zHDwZtv/WcJRIQI+h
wwSJCDS0U0dlb3JnaW9zIEtvbnN0YW50b3BvdWxvcyAoUmV0aCBzaWduaW5nIGtl
eSBmb3IgMjAyNCBhbmQgb24pIDxnZW9yZ2lvc0BwYXJhZGlnbS54eXo+iJMEExYK
ADsWIQRQ+3zFWy6K+ln+A7eqXtVqf78lPgUCZl4GjgIbAwULCQgHAgIiAgYVCgkI
CwIEFgIDAQIeBwIXgAAKCRCqXtVqf78lPtg6APwJXCdEG3OCrYTbOIWtLs5cdFlu
UqqUy9J/6Frn7Ss/lwD+PtqDy6AbpX83IcdlSU2cDQQkZWOHG1JPsK33l1lieQy4
OARmXgaOEgorBgEEAZdVAQUBAQdApFaGkJqDMd9RMuAlQVbqWy23w3TxSTHS4Oy8
dD7tvUIDAQgHiHgEGBYKACAWIQRQ+3zFWy6K+ln+A7eqXtVqf78lPgUCZl4GjgIb
DAAKCRCqXtVqf78lPlR7AP42Qr+RGsdneH73y2yd26sJpUvRoQ/IcbNMXmxAU3YZ
zwEA/K0/Im6d1n9d7fjE9fHh4gjNwZufzVTMJhX6byOo/wM=
=zczG
-----END PGP PUBLIC KEY BLOCK-----
```
