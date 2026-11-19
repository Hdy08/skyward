# MIX Fold 2 dual boot

Skyward supports one additional system beside the installed Android system.
The second system can use Android's native DSU image service or the existing
Linux package format. Installing one kind requires removing the other first.
Selecting a second system uses the existing confirmation after installation.

## Basic Android DSU installation

As requested on 2026-11-17, Android support is limited to installing prepared
DSU images through the installed `gsid` service. Skyward allocates independent
userdata and streams each supplied image unchanged. The installed first stage
is responsible for booting it, using the main system's kernel, vendor and
firmware unless the prepared DSU package supplies a supported image itself.
Framework/vendor compatibility is the responsibility of the supplied images.

Open Data, then choose **Dual boot → Install a system** and select a prepared
ZIP containing `android.toml` and stored, uncompressed raw images. The manifest
names the system and image sizes; for example:

```toml
id = "android-dsu"
name = "Android DSU"
version = "15"
device = "zizhan"

[images.system]
size = 1073741824
sha256 = "<the image's 64 hexadecimal SHA-256 digits>"
```

Replace the example size and digest with the actual values. The ZIP entry is
`system.img`, and its size must match the manifest. Additional prepared images
may be named `system_ext`, `product`, `mi_ext`, `vendor`, `odm`, `vendor_dlkm`,
`odm_dlkm` or `system_dlkm`. Supplying one explicitly does not cause Skyward to
derive it from the main Android or modify it. Raw images must be block aligned.

The original capacity-selection and slide-to-install flow is retained. The
minimum independent Data size is 8 GB; system images consume additional space.
The existing free-space reserve for a main-system update and 1 GiB remains.
Installation finishes with the main system selected. The Dual boot page can
select the installed DSU, return to the main Android, or remove the recorded
second system. No extra preverification or readback page is added.

Official full OTAs are not converted into DSU packages. They are refused when
chosen through Dual boot; the ordinary **Install** entry continues to update
the main Android. The selected destination survives internal-storage, USB-drive
and sideload selection and retries. Leaving the second-system installation
methods clears that destination. Scripts and partition images are also refused
through the second-system entry.

## Native service and state

New installs use Android's default `/data/gsi/dsu` location. Existing records
with named child directories retain those locations for switching and removal.
The native image service owns the image mappings and separate userdata.

Older services that finish image writes without publishing a disabled
installation have their completed native metadata registered in that state.
The main system stays selected. State in `/metadata/skyward/systems.toml` is
written before allocation so interrupted installs remain removable. Existing
unrecorded dynamic systems are not overwritten.

Recovery clears Android's stale DSU `booted` marker only during an explicit
mutation, after checking that no DSU filesystem is mounted. Removal uses the
installed image manager for the recorded slot because Recovery does not run
Android's vold service. Native boot-attempt status is displayed accurately so
a failed first attempt can be selected again through the existing confirmation.

## Removed ROM adaptations

The following earlier experimental behavior is removed from both the Recovery
and its build tools:

- Full-OTA extraction for second Android installation, including the
  `skyward-pack android-system` converter.
- EROFS-to-ext4 conversion, generated AVB trees and descriptors.
- Automatic vendor copies and vendor fstab rewrites.
- Private Android init replacement and `mi_ext` mount aliases.
- Fold-device property defaults, display-layout translation and backlight
  capability overrides for MIUI/HyperOS combinations.

The previous v34 ROM-combination results apply to the removed implementation.
They do not establish that those official ROMs boot through the basic DSU
installer. Existing installed images are not rewritten or deleted by updating
Recovery: those images may still contain earlier experimental modifications.
Installing a clean DSU requires removing that installation through the existing
Dual boot page and providing an unmodified prepared package.

## Linux packages

The existing Linux path is unchanged. Its ZIP contains `system.toml`, a
compatible `boot.img`, a raw `rootfs.img`, and optionally `init_boot.img`.
MIX Fold 2 has paired boot partitions and no physical init_boot partitions, so
the package kernel/header and startup ramdisk use its boot-only layout. The
fixed Data area, boot store, journaled switching and original boot restoration
remain part of that existing path.

The Linux startup probe previously reached PID 1 in its independent root and
returned to Recovery; selecting the main Android restored its original boot.
No complete Linux distribution was supplied or validated.

## Validation scope

The basic DSU implementation retains package-format, native state, install
destination and UI regression tests. A valid prepared DSU image compatible with
the installed kernel/vendor is required for a new installation and boot test.
The old MIUI 13 plus OS3 test installation is not a clean basic-DSU test image.
Build, image and device checks are recorded with the corresponding artifact.

The v35 basic-DSU Recovery is 104,857,600 bytes, SHA-256
`cced607e17dc262fd684ab3dd104f646d47eca22be60d8b01c7ff7cba3371706`.
Forty-one related Android-native tests and fifteen host packing tests pass,
as do Android workspace Clippy and independent image checks. The removed
Android init, fold/layout and brightness patch markers are absent from both
the Recovery program and static Linux startup helper; the desktop converter
is absent from `skyward-pack`.

It was written to recovery_b with a matching full readback; the running
program matches the build. Both physical boot sets and recovery_a retain
their prior hashes. The owner confirmed startup, inner/cover display,
folding, touch, Data access and the existing-system list. ADB reconnected
after toggling file transfer. The existing MIUI 14 V14.0.14.0.TLRCNXM / 16 GB
DSU record and metadata encryption keys remain unchanged; its images were
not rewritten. A clean basic-DSU installation and boot have not been tested.
The phone remains in Recovery B v35. Source changes are uncommitted.

An earlier investigation recorded `rtsar_efs.c:214` / `LMTSMGR` modem assertions
in both the main MIUI 13 and the experimental second Android. That investigation
does not validate radio stability for a clean DSU or resolve the assertion.
