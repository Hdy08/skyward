# MIX Fold 2 dual boot

Skyward supports one additional system beside the installed Android system.
The second system can use either the existing Linux package format or the
Android image-service format described here. Installing one kind requires
removing the other first. Selecting a second system requires a separate
confirmation after installation.

## Android packages

Convert a full official OTA on the computer:

```powershell
.\target\release\skyward-pack.exe android-system `
  --package 'D:\Flash\zizhan_XiaomiMIXFold2\Rom\zizhan-ota_full-OS3.0.9.0.VLRCNXM-user-15.0-30ba6b19a5.zip' `
  --config .\devices\zizhan.toml `
  --out 'D:\Flash\zizhan_XiaomiMIXFold2\Rec\Skyward-Android-zizhan-OS3.0.9.0.zip'
```

The converter checks the device and full-payload metadata, extracts system,
system_ext, product and mi_ext when present, and verifies their payload hashes.
It writes a stored ZIP containing `android.toml` and raw partition images,
plus a SHA-256 sidecar. The original OTA remains an ordinary system-update
package in Skyward; choose the converted ZIP to install a second Android.

Open Data in Skyward, choose the converted ZIP through Install, select the
capacity for its independent apps and data, then slide to install. The system
images consume additional space. Capacity choices preserve a super partition's
worth of free space for main-system updates, plus 1 GiB and the working room
for any required compatibility copies and their temporary conversion files.

The installer allocates through the installed Android `gsid` service and
streams each image through Binder, using the existing installation confirmation
and progress flow. It does not add a whole-image preverification or an extra
readback stage.

The installed Android 12 first stage creates an ext4-only mount entry for
the extra `mi_ext` image. During the normal install, Skyward converts an EROFS
`mi_ext` to ext4 while preserving its files, owners, modes, timestamps, SELinux
labels and other extended attributes. The current vendor's fstab would then
bind the main system's `mi_ext` over that image. Where this layout is present,
Skyward also copies the installed vendor filesystem into the DSU, reversing
only that mount direction in the copy. Xiaomi's Android init also loads
`/mnt/vendor/mi_ext` properties and init scripts before `mount_all`, while the
old first stage has mounted the DSU image at `/mi_ext`. Where this layout is
present, Skyward prepares a private ext4 system image whose init entry binds
the DSU image to that early path before executing the unchanged Android init.
The original init is retained with its ownership, permissions and extended
attributes beside that entry. The entry binds the retained executable over
itself before SELinux setup, so Android's self-exec goes directly to its own
init and retains the normal SELinux transition. The second system uses its own `mi_ext`
properties, init scripts and overlays throughout startup. Firmware and other
child mounts are not included in the vendor copy. Staging images use unnamed
temporary files. Closing their last handle releases them; rebooting also
releases any loop mount held when the installer was interrupted.

The main system remains selected when installation finishes. The Dual boot
page selects a system and reboots after confirmation. Returning to Recovery
allows selecting the main system or deleting the second system.

This Android combination uses the installed kernel, vendor hardware files,
dtbo and firmware. The physical boot, vendor_boot, vendor and dtbo partitions
are not changed by an Android switch. It does not install the official
package's kernel or firmware, repartition super, or share the main Android
userdata filesystem with the second Android.
Cross-version framework/vendor compatibility still requires a real boot test;
successful conversion alone does not establish that a ROM can start.

The state in `/metadata/skyward/systems.toml` is written before allocation.
Interrupted installs remain recorded and removable. An existing dynamic
system outside Skyward is not overwritten. Recovery clears Android's stale
DSU `booted` marker only during an explicit mutation, after checking that no
DSU filesystem is mounted. Deletion uses the installed image manager for the
recorded slot because Recovery does not run Android's vold service.
Native DSU can remain enabled after exhausting its first boot attempt. The
page distinguishes that state from a system that will actually start, allowing
the second system to be selected again through the existing confirmation.

## Linux packages

The existing ZIP format contains:

- `system.toml`: id, name, version, filesystem, init and optional minimum size.
- `boot.img`: a kernel compatible with this device and its installed vendor
  ramdisk, modules and dtbo.
- `rootfs.img`: a raw filesystem with the declared executable init.
- Optional `init_boot.img`: a first-stage ramdisk supplied by the system.

MIX Fold 2 has paired boot partitions and no init_boot partitions. Skyward
keeps the package kernel/header and places its startup program in the boot
ramdisk. An explicitly supplied first-stage ramdisk is preserved ahead of
Skyward's mapping helper. The fixed Data area, boot-store hashes, journaled
switching and original boot-image restoration apply to this layout too.

Recovery mounts unencrypted Data through an identity device-mapper mapping,
allowing Android's image service to share the underlying userdata partition.
The filesystem contents and encryption settings stay the same. If an existing
mount directly claims the raw partition, the Linux area mapper can instead open
a writable, direct-I/O loop view before creating its linear mapping. The saved
physical extents and the plaintext area format stay the same.

The Linux package must include compatible device support. A generic desktop
root filesystem alone is not a complete MIX Fold 2 port. No Linux distribution
package was supplied for this task; validation status must distinguish the
startup/switching framework from a complete Linux distribution.

## Validation

Validated on 2026-11-16 with the reference phone's custom
OS3.0.8.0.VLRCNXM main system, Android 15 framework, Android 12 vendor interface
and unencrypted F2FS Data. There was approximately 399 GiB free before copying
the Android package. A small Linux
startup probe was installed through the normal package flow, selected through
Dual boot, and started as PID 1 inside its independent ext4 root. It recorded
success and automatically returned to Skyward. This validates the Linux root
handoff on the boot-only layout; it does not establish support for a complete
Linux distribution.

Selecting the main system restored the original boot image and booted
OS3.0.8.0.VLRCNXM. The owner confirmed that the existing apps and files remained
available. Both boot slots then matched the original SHA-256
`a18547f9e51ae4b02e018daa25006e119cd25956694bc986fe224f7aa95aacac`.

Android image-service discovery, persistent Binder references and image streams
work in Recovery. The converted OS3.0.9.0 package installed successfully with
32,000,000,000 bytes of independent userdata, using the normal confirmation and
progress flow. The native service reported a complete installation with the
main system still selected. Compatibility images were then prepared through
the production conversion functions and substituted without reallocating
userdata. The final DSU has the official system, system_ext, product and mi_ext
contents, with the init handover described above and the private copy of the
installed vendor. The owner confirmed that it boots to a working Android
session, About phone displays OS3.0.9.0, and existing second-system settings
remain available. Both `ro.build.version.incremental` and
`ro.mi.os.version.incremental` report OS3.0.9.0.VLRCNXM.

A normal reboot again reached the second Android with native DSU status
`running`, `sys.boot_completed=1`, the independent 32 GB F2FS Data mapping,
preserved ADB authorization and SELinux Enforcing. Returning to Recovery and
selecting the main system booted OS3.0.8.0; the owner again confirmed the
original apps and files. The second Android remains installed and disabled,
ready to be selected again. Final boot_a and boot_b hashes match the original
hash above; vendor_boot_a, dtbo_a and recovery_b also retain their originals.

The Recovery image used for this dual-boot validation was 104,857,600 bytes, SHA-256
`d00ca9858e7c4fa4a60e3888d8527a5be37249f05d7384fdd04a026c66363fe7`,
and matched recovery_a at that stage. The former non-dual-boot delivery is
retained separately. Subsequent unencrypted-Data backup and file-manager/display
fixes are documented in [the device notes](zizhan.md#data-backup-and-display-recovery).

The converted OS3.0.9.0 package is 6,405,018,933 bytes, SHA-256
`addc7c9ecf42fd8bd4c5c87a768873f827a7f9e945c93c1037368da9bcb6f0ca`.
Other framework/vendor combinations and complete Linux distributions have not
been validated by this result.
