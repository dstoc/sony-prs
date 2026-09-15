# Historical artifacts and references

These are the resources to obtain and inspect when network access to the old
archives is available:

- [PRS+ source](https://github.com/kartu/prs-plus)
- [MobileRead Sony Reader hack protocol notes](https://wiki.mobileread.com/wiki/Sony_Reader_hack)
- [MobileRead `ebook.py` 0.41 file page](https://wiki.mobileread.com/wiki/File:Ebook_py_041.zip)
- [MobileRead PRS-650 archive](https://projects.mobileread.com/reader/users/porkupan/PRS650/)
- [MobileRead PRS-350 tools archive](https://projects.mobileread.com/reader/users/porkupan/PRS350/tools/)
- [MobileRead PRS-350 flash packages](https://projects.mobileread.com/reader/users/porkupan/PRS350/flash_packages/)
- [Historical discussion of PRS x50 kernel sources and extended SCSI](https://www.mobileread.com/forums/showthread.php?nojs=1&p=1138611)

The specific files of interest are:

```text
ebook_msc.1.06.zip
PRS350_update_tools.zip
PRS350.Flash.Package.orig.zip
Ebook_py_041.zip
```

The old `ebook_msc` executable is useful as a behavior oracle, but its
Windows-side source and Sony communication DLLs are not required by this
project. The higher-value missing artifact is Sony's x50 GPL source containing
the device-side `SC_SONY_EXTENDED` implementation, reported under names such
as `usbtg_ebook5.c` and `usbtg_ebook5_gfile.h`.

## Shell-trial package provenance

The package used for the non-flashing shell trial came from the historical
`PRS350_update_tools.zip` archive in the [MobileRead PRS-350 tools
directory](https://projects.mobileread.com/reader/users/porkupan/PRS350/tools/).
The reproducible download helper uses this direct URL:

```text
https://projects.mobileread.com/reader/users/porkupan/PRS350/tools/PRS350_update_tools.zip
```

The local archive was 42,798 bytes with SHA-256
`0c1f44fdc6d5cd18d236d43093bec89d51fb8c55c4a9e22a2bbacec7991c4e7c`. The
selected file was the unmodified
`PRS350_update_tools/login_update/PRS-350 Updater.package`, 11,280 bytes,
with SHA-256
`6a35343c7fe4a27b8f32dca31a05d05927dfe7e81205bca25666c04188846f0a`.

This is a community-hosted historical archive, not an official Sony download.
“Signed” refers to the legacy Sony updater package signature: the recovery
image's own verifier accepted the package checksum and RSA signature when
given the matching device `Info.img` key material. No package bytes were
modified for the trial, and private key material is not stored in this
repository.
