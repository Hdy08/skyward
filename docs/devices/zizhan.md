# Xiaomi MIX Fold 2 (`zizhan`)

This is a local stock-recovery port for the owner's MIX Fold 2. A third-party
recovery is not required. Both panels, automatic inner/cover switching and
preservation of the screen-off state have passed owner validation on the
reference ROM.

The reference device is `22061218C` with an owner-supplied custom Android 15 ROM
reporting `OS3.0.8.0.VLRCNXM`, system patch `2026-06-01`, Android 12 vendor
interface 31, vendor patch `2026-02-01`, and a 5.10 GKI kernel. These are the
installed custom ROM's reported versions, not a claim of an official Xiaomi
release for this device.

## Local base and backups

`devices/zizhan.toml` pins the complete 100 MiB `recovery.img` from the local ROM
with SHA-256:

```
e10f55fad2f218a26c347cb0d9d0e2cd3c39a0cbdc3fe81effec58d21e62c60e
```

Both original recovery partitions read from the device have this same digest.
The image uses boot header v4 and has no kernel; the installed kernel and vendor
boot are retained. The packer preserves the base's init, adbd, libraries, policy,
and board initialization. It replaces the recovery executable, adds Skyward's
configuration and resources, and generates the recovery partition's AVB footer.

Before the first flash, `recovery_a`, `recovery_b`, `boot_a`, `vendor_boot_a`, and
`dtbo_a` were copied to the local workspace and to the owner's resource directory
under `Rec/Skyward-backup-20261115`. Each copy was checked against the SHA-256
read from its source partition. The manifest next to those files records their
sizes and digests. The first test writes only `recovery_a`, after Fastboot
reported product `zizhan`, unlocked `yes`, slot `a`, and partition size
`0x6400000`.
Subsequent porting tests also write only `recovery_a`.

## Android 12 recovery compatibility

The base lacks `servicemanager`. For this case the packer adds a disabled init
service and a marker; startup takes the installed service directory executable
into recovery RAM and starts it with its installed runtime and loader. The
installed system's partitions remain mounted read-only. The reference system's
Binder libraries use the `SYST` parcel header.

The installed credential and boot control services use HIDL, while KeyMint uses
AIDL. The existing service discovery reads the vendor's init and VINTF files and
selects the `cape` hardware description through its own `init.qti.qcv.sh`.
The WLAN supplicant's HIDL declaration is supplied to the hardware service
directory rather than published as an AIDL service.

With this GKI, legacy `/dev/hwbinder` is absent and the old recovery's init does
not populate binderfs. Recovery creates the missing Binder devices with
`BINDER_CTL_ADD`, using the kernel-assigned device numbers.

The vendor's Android 12 Binder libraries and the installed Android 15 framework
libraries must remain in separate linker namespaces. Vendor programs, including
the credential services, sensor programs and WLAN supplicant, run with an
installed loader and a generated linker configuration inside private child
mount namespaces. Recovery and its other
programs retain their original loader. Installed `/vendor/ueventd.rc` rules are
applied again after module loading so newly created QSEE and DMA heap devices
have the vendor's intended owners and access modes.

## Startup and touch firmware

The stock board init aliases the modem filesystem at `/vendor/firmware`.
Preserving this submount when the installed vendor is mounted hid the vendor's
touch firmware. The compatibility init script removes this alias before the
installed vendor is mounted. The modem firmware remains available at its
normal modem mount points.

With the vendor's own `st_fts_l18.ubin` and `st_fts_l18_sec.ftb` visible, the
inner touch driver completes its probe and reports matching E455 firmware;
it does not need to update the controller. The eight-second startup workaround
has been removed. The owner observed a complete boot with only one startup,
directly into Skyward. Earlier boots showed a CFI failure in `call_timer_fn`;
the exact owner of that failing timer was not established. Trace events are not
enabled in the image.

## Displays, input and folding

Both DSI connectors report connected whether the phone is folded or unfolded.
The cover display is `DSI-2`, 1080 by 2520, with `panel1-backlight` and touch
input `fts-sec`. Its corresponding panel parameter is `dsi_display1`. The raw
backlight maximum is 4095; the panel's device tree exposes no separate normal
maximum, so this recovery configuration gives the slider its explicit range.

The inner display is `DSI-1`, reports 2160 by 1916 modes, and uses
`panel0-backlight` and `fts-pri`. Its framebuffer is rotated 270 degrees into a
1916 by 2160 upright UI; touch coordinates are already upright. Its normal
brightness ceiling is 2047. The owner confirmed the corrected inner display
direction and menu touch positions.

The touch modules are `fts_touch_spi_pri` and `fts_touch_spi_sec`; dependencies
come from the installed `vendor_dlkm`. The optional touch-processing programs
listed in the vendor init script are absent on this installed system, and are
not added as required recovery services.

During Android-side calibration, `SW_LID` remained zero in both positions while
Android's device state changed between its inner and cover displays. The ROM's
state provider uses the vendor `fold_status FOLD_STATUS Wakeup` sensor. Recovery
therefore reads this SSC sensor through the installed QMI client libraries.
The sensor UID is looked up at runtime. Only the first sample value is used:
1 means closed and 0 means open. The owner performed a close/open cycle, and
both states were recorded correctly.

SSC needs its writable sensor registry before the sensor processor starts.
Recovery mounts persist read-only, copies its `sensors` directory into RAM,
and binds that copy at `/mnt/vendor/persist/sensors`. It starts the installed
diag-router, pd-mapper, sscrpcd and sensor registry programs with their vendor
accounts before requesting SLPI boot. Registry changes remain in RAM; the
calibration partition is not written. Existing sensor programs and the RAM
registry are retained if the UI process restarts. In particular, the registry
server must not be restarted while SLPI is initializing.

New registry mount-point parents are traversable by the vendor system account,
and sscrpcd receives the vendor's wake-lock group and suspend-blocking capability.
The fold monitor starts while the credential services are still coming up,
before background WLAN preparation, so its discovery overlaps the remaining
startup and its private namespace does not inherit WLAN's temporary calibration
mount. For a normal menu boot, fonts, page resources and the initial composition
are prepared while the boot splash remains visible. The UI waits up to 2.5
seconds for the monitor's first state before taking the display, selects the
corresponding panel, and presents the first frame before starting USB and WLAN
background work. The wait ends as soon as the sensor event arrives; a missing
event leaves the cover usable and subsequent sensor events still select panels.

The fold client runs in a separate process with the private vendor runtime.
The UI keeps one DRM client and retains each panel's separate CRTC, planes,
mode blobs and framebuffers while switching. An inactive panel rests in its
low-power mode and wakes through the same resume path as the power key.
Removing its last framebuffer tears down its display route, and cycling power
at every fold intermittently left a panel dark despite successful commits.
The UI selects the active panel's named touch controller and reflows the
current page. Page navigation, text fields and background work remain in the
same process. Display power is preserved: folding a dark phone selects its
next panel without lighting it, including during the fade to black. One press
of the power key wakes the selected panel. Touch gestures
and unfinished confirmation gestures are cancelled at a switch. A running
terminal receives its new size without replacing its shell. During startup,
both backlights are first darkened, then the other panel receives a black frame
through its own initialized KMS surface and is parked in low-power mode. This
replaces its boot splash before the first fold, while retaining its resources
for later switches.

The owner confirmed eight successive fold operations without black screens or
corruption after the low-power transition fix. The owner also confirmed that
folding after eight seconds with the screen off preserves the dark state and
one power-key press lights the correct panel with its page and touch intact.
A closed-device boot entered Skyward directly on the cover. After the startup
change, the owner confirmed that the inner boot-logo residue disappeared,
switching and touch remained correct, and screen-off switching and power-key
waking still worked.

## Vibration and large-frame rendering

The stored click uses a 6667 us pattern period (about 150 Hz), with automatic
resonance disabled. The motor reports 1954 in tenths of a hertz, or 195.4 Hz.
The port now generates the click at that measured resonance, with four drive
cycles followed by the existing brake, about 28 ms in total. The owner's A/B
test and menu check confirmed clearer feedback at a suitable strength. Other
devices retain two drive cycles unless configured otherwise.

The inner panel scans out 2160x1916 pixels, while the upright UI is 1916x2160.
The driver advertises inline RGB rotation only for Qualcomm-compressed buffers,
including `AB24/5/1`; it does not list the linear RGB dumb buffers used by the
CPU renderer. Removing the CPU rotation requires a different output path or a
renderer that writes directly in the panel's native layout.

The existing path now transposes four pixels from four rows with AArch64 NEON,
in 128x128 tiles, assembling 64 contiguous output bytes before storing
each row. The direction and colour order are specialized outside the loops.
Tests cover rotation, colour order, alpha, padding and
dimensions that cross SIMD and tile boundaries. The About field and alpha
blending also use exact integer NEON operations, fusing translucent cards with
the field before storing the output. Nine complete inner-sized
frames at different fades and scroll offsets matched the previous renderer
byte for byte.

An unattached real DRM dumb buffer measured about 5.52 ms for the original
scalar inner-frame copy and 1.00 ms for the optimized copy. These figures
measure only the copy, not the complete UI or its presentation rate.
The status-bar mask also blends eight pixels at a time, preserving its
four-step opacity dither, rounding and source alpha; 234 diagnostic masks
matched the former output byte for byte.

Large Android About frames use a private pair of persistent painting workers.
The workers request CPU capacity while runnable. The calling UI thread keeps
its capacity hint while the large About page is visible, then restores the
previous value when another page, the cover, or a dark display is selected.
Unsupported scheduler hints fall back to a single painting thread. The normal
scheduling policy and priority are retained.

On the reference device, twelve one-second DRM samples during continuous inner
About-page scrolling averaged 119.675 fps, with a 118.2 to 120.5 fps sampling
range. The same sampling method measured 72.325 fps before these rendering
changes. This is approximately 120 Hz presentation, rather than a guarantee
that every frame meets its deadline; an individual interval can report slightly
over 120 because of the measurement boundaries. The owner confirmed smooth
scrolling, suitable menu vibration, six to eight fold cycles without black
screens or corruption, and unchanged screen-off folding and power-key waking.

## Data

The installed custom ROM declares neither metadata encryption nor file
encryption for userdata. A read-only probe found a directly readable F2FS
filesystem, no fscrypt policy on the owner's media directory, and no vold
encryption key directories. Skyward now accepts a directly readable volume
when the installed fstab declares no metadata encryption. It keeps the normal
key-based path when encryption is declared or file-encryption records exist.

Data passed the requested filesystem check, mounted automatically, and the
owner confirmed that internal storage was visible in the file manager. No
screen credential was needed. This validates access to this ROM's unencrypted
Data; encrypted Data decryption has not been validated on MIX Fold 2.

## Dual boot

The Linux boot-only layout and separate Android image-service installation are
described in [MIX Fold 2 dual boot](zizhan-dualboot.md), including package
conversion, storage layout and current device-validation results.

## WLAN and updates

The WLAN module's internal name is `qca6490`; its actual loadable filename is
`qca_cld3_qca6490.ko`. It uses CNSS `fs_ready` startup and firmware supplied
through the stock modem image. The vendor's Wi-Fi configuration comes from
`/vendor/etc/wifi/qca6490/WCNSS_qcom_cfg.ini`; the MAC file actually present on
this device is `qca6490/wlan_mac.bin` on persist. Its vendor firmware symlink to
`wlan/wlan_mac.bin` is dangling on the reference ROM.

The installed supplicant registers Android HIDL 1.4 and the Qualcomm vendor
HIDL 2.2 interface. Driver preparation runs in the background at boot, leaving
the interface down until the owner enables WLAN. The owner confirmed that
WLAN became markedly faster and could connect to the Internet. On the measured
boot, background preparation took about nine seconds; enabling after that took
about 110 ms and the first scan about 1.3 seconds.

The owner also confirmed computer access to internal storage over MTP, with
ADB remaining available at the same time.

The update configuration selects Skyward's existing Xiaomi query provider.
The owner confirmed a query result for `OS3.0.9.0.VLRCNXM`. Downloading or
installing that package has not been tested on MIX Fold 2. A successful query
does not establish installation compatibility over this custom ROM.

## Data backup and display recovery

Unencrypted Data does not keep `/data/unencrypted/ref`. Backups now record
no shared file-key reference in that case, and restoration compares the same
optional identity. An encrypted installation still requires its reference,
and other read failures remain errors. The existing backup manifest format
and encrypted-key encoding are unchanged. The reference is read before
archiving, so a missing required record fails before writing a large backup.
File-encryption detection is shared with the Data opener.

Data backups also leave `/data/gsi` out, along with the existing Linux area
and internal-storage exclusions. A Data restore leaves those second-system
images as they are.

A file-manager row tapped near the viewport edge could extend outside the
drawn band. Cropping its pressed-state artwork then asserted and aborted
the UI process. Drawing that row's full extent before cropping fixes the
failure. A regression test reproduced the assertion with a tap inside the
cover panel, and now covers both MIX Fold 2 panel sizes and several filenames.
After a service restart, display encoders can also lose their routing. Panel
initialization now chooses a CRTC that no other connector occupies, preventing
preparation of one display from disconnecting the other. Panic messages are
retained in the Recovery log for diagnosis.

Native Android tests passed for the backup worker (3), backup archive/store/
manifest (9), file manager (20), and page drawing (8). The manifest test covers
both encrypted and unencrypted Data. Android Clippy with warnings denied and
the independent image checks passed. A controlled Recovery-service restart
retained separate nonzero CRTC routes for both displays.

The normal UI then completed a Data-only uncompressed backup on the reference
ROM: 18,146,092,032 bytes across 17 archive volumes, saved in
`/sdcard/Skyward/backups/20261116-030434`. Its format-1 manifest was written
successfully with no `data_key`; all listed volumes exist, and their combined
size matches the manifest. `/data/unencrypted/ref` remains absent. The owner confirmed that deleting
the test file, repeated folding and screen-off/power-key wake work normally.
The UI process remained the same throughout those operations and the backup.
No full Data restoration was performed during that unencrypted-ROM check.

The Recovery containing these fixes is 104,857,600 bytes, SHA-256
`391dff455b12b4420e8f3e841b8644ece50772961046862888987bc09e456a74`,
and matches recovery_a. Final boot_a, boot_b, vendor_boot_a, dtbo_a and recovery_b
hashes retain their originals; the dual-system state matches the state recorded
before this update.

## ADB sideload mount cleanup

The reference device received and identified the official
`OS3.0.9.0.VLRCNXM` package, but stopped before applying it with
`move off the installed system: remove device-mapper device vendor_dlkm_a:
Device or resource busy (os error 16)`. Ten vendor runtime namespaces still
had the read-only `/vendor_dlkm` mount after the recovery's own copy had
been unmounted. They kept its device open without holding any module files.

Partition release now unmounts unused copies in other mount namespaces
before removing the corresponding device-mapper mappings. Each namespace
is visited once by a temporary thread with a separate filesystem context;
the UI and other workers retain their namespace. Mount inspection uses
`/proc/thread-self/mountinfo`. Nested firmware and RAM mounts are preserved.
Filesystems that remain in use still fail to unmount rather than having
their removal forced. The original confirmation and installation flow is
unchanged.

Two native regression tests exercise a real ext4 filesystem over a temporary
loop/device-mapper device. They cover inherited mounts in two private
namespaces, child RAM mount preservation, repeated cleanup, caller isolation,
successful final device removal, and refusal while a private process still
uses the filesystem. Both passed on the phone without changing the recovery
process or its mounts. Nine ADB protocol and nine FUSE block-serving tests
also passed, as did Android Clippy with warnings denied and the independent
image checks.

The normal UI then received and ran a harmless script package twice over
`adb sideload`, including the result page's send-again action. Both runs
passed the original failure point, with `vendor_dlkm` absent from all
process mount namespaces, normal script completion, and MTP/ADB restored.
The second package also verified the logical partition mappings before
script cleanup. The recovery process remained the same. Guarded boot,
vendor_boot, dtbo and recovery_b hashes and the dual-system state were
unchanged. A complete official OTA installation was not performed.

The Recovery containing this fix and the preceding Data/display fixes is
104,857,600 bytes, SHA-256
`810dc2a881ad9e64293354d5236669e6663d599859e11cde600e24711c028a12`.
After restarting this image, the owner confirmed normal inner/cover displays,
touch, Data access and computer file transfer. The original Data backup and
its 17 archive volumes remain present with the same manifest and sizes.

## Unfolded startup on official OS3.0.9.0

On the installed official `OS3.0.9.0.VLRCNXM` in slot B, an unfolded boot
showed the inner page briefly, went black, and repeated until the phone was
closed and opened. The saved recovery log contains four UI panics at
`Canvas::join`: a new band was 1916 by 2657 while the cached home page was
1916 by 2504. These were recovery process restarts within one kernel boot.

The first foldable frame was cached before the WLAN worker started. Its
startup then added the WLAN entry by assigning page state directly, without
invalidating that cached layout. On the shorter inner screen, drawing the
remaining rows reached the mismatched page heights. WLAN availability and
the online update capability are now initialized before the first cached
frame; worker startup no longer changes the home page's structure. This
retains the existing early display handoff and does not change panel power
or folding behavior.

A separate saved boot had reported `read how wlan0 stands: No such device
(os error 19)` during background preparation. Driver preparation now waits
for that specific transient ioctl error within the same 60-second deadline
used for interface creation. Other errors still return immediately. There
is no added delay when the interface is ready. This addresses the observed
interface readiness race; the owner's single earlier enable failure was
not captured with a complete contemporaneous log.

Native Android tests passed for page drawing (9), display resizing/power
(2), WLAN UI (8), and interface readiness/error handling (4). The new home
page regression compares complete pixels and touch targets after banded
drawing on both MIX Fold 2 panel sizes, with WLAN available before or after
the first frame. The four runtime tests cover transient ENODEV followed by
success, immediate success, another error without retry, and deadline
expiry with the original error retained. Android Clippy with warnings
denied and independent image checks passed.

The fix was written to the active `recovery_b` and read back. The owner
confirmed a direct unfolded boot with no repeated black screen, normal
folding/touch/page retention, and three WLAN off/on cycles followed by a
working Internet connection. The UI remained PID 660, with one startup and
no panic or panel-switch failure. The log records four successful WLAN
starts: 1625, 114, 118 and 118 ms. Background preparation completed in 9355
ms. The final boot, vendor_boot and dtbo hashes for both slots and the
inactive recovery_a match the values captured before this change.

The image is 104,857,600 bytes, SHA-256
`db7df16d32ac9f000d2649cb2ad3e51b1fd5079d5f220975f0e18e2552ee2c22`.
It includes the preceding dual-system, Data backup, display and sideload
fixes. No Android installation or system-slot change was performed during
this startup fix.

## Sideload transfer and application progress

Full packages may have earlier build and security patch dates. Package
identity, serial targeting and incremental-package checks are unchanged.
The dates do not add a wipe request or modify payload headers; explicit
package wipe requests retain their existing confirmation. Recovery already
enables the stock update engine's support for earlier build timestamps.

The writing percentage previously used the highest file offset reached by
the sideload FUSE server. ZIP identification reads the central directory at
the end of the file, so the writing page could show 100% before payload
application. It now follows the engine's logged application percentage.
The reference engine formats its status-pipe `set_progress` with `%.lf`,
rounding to 0 or 1; its detailed log preserves the actual percentage. The
existing writing, readback and finalization pages remain the same, and late
messages from one output cannot move a later phase backwards.

USB reads are sized for the missing header or complete payload, up to the
negotiated maximum, rather than splitting a usual 64 KiB block into four
synchronous 16 KiB requests. Protocol tests cover USB fragmentation,
coalescing, zero-length packets, acknowledgements, legacy checksums and
block-length errors. The same 128 MiB read-only script package establishes
an observed baseline of 134,218,502 bytes in 12.775598 seconds. This is a
transport measurement rather than a complete official OTA installation.

Native Android regression tests passed for ADB (13), FUSE (9), update-engine
progress (7), package policy (14), wipe-package policy (4), installation UI
(16), stage text (1), and refusal text (1): 65 tests in total.
The tests use `/tmp` for scratch files; they do not require mounting Data.
Android Clippy with warnings denied and independent image checks passed.
The recovery was written to the active `recovery_b` and read back; the owner
confirmed normal display and touch. The identical read-only package then
completed in 10.475555 seconds (12.22 MiB/s), versus 12.775598 seconds
(10.02 MiB/s) before the change, approximately 22% higher throughput.
The authorized official OS3.0.9.0 full package (6,467,751,457 bytes) was then
installed through sideload in 858.36 seconds (14 minutes 18 seconds), with
ADB exit code 0 and total transfer 1.00x. Package size divided by this full
session is 7.19 MiB/s, including application, partition readback and
finalization, rather than a separate USB throughput measurement. The writing
log advances from 0 to 100 with intermediate values including 16, 19, 20,
23 and 27, followed by readback of 34 partitions and successful postinstall.
The owner confirmed the completion page and a normal boot into OS3.0.9.0
with existing applications and files. The percentage follows the engine's
discrete reports; it does not interpolate an artificial smooth percentage.

This real install exposed a further retention issue: update_engine left
the target `recovery_a` block device read-only, so writing Skyward back
failed with EPERM. Image flashing now temporarily clears block-device
read-only protection and restores its original state on both success and
error. Eleven image tests, four retention tests and four recovery-upgrade
tests passed on Android, for 84 relevant native tests across this change.
Two of the image tests use temporary file-backed loop devices and exercise
both protection states plus failed source reads and failed target opens.

The final v15 image was independently verified and written to the actual
read-only `recovery_a` using the corrected `image::flash`, then read back.
Its original read-only flag was restored; the other recovery and both
slots' boot, vendor_boot and dtbo hashes were unchanged by this repair.
The full official reinstall above used v14; v15 adds the retention fix and
was not used for a second full reinstall. After the Android boot, v15
started from slot A with the expected executable hash, one UI startup and
no panic. The owner confirmed startup, both displays, folding, touch,
WLAN, Data access and computer file transfer again.

The final image is 104,857,600 bytes, SHA-256
`d5d099dcb3a8dfdbc9c711e747d45b5f2eeec350136afc0cb488de5d75a2b8ae`.
The current source removes both recovery-level age restrictions; an actual
downgrade to an older Android release has not been tested.

## Metadata erase

Factory reset previously erased Data, then failed while formatting metadata:
`mke2fs` saw `/dev/block/by-name/metadata` in use even though the recovery's
own `/metadata` mount had already gone away. Vendor key and fold services had
kept private mount-namespace copies of the metadata filesystem.

Before recreating any fstab partition, the recovery now finds the partition's
device number, unmounts copies in inherited private namespaces, and removes
the recovery's own mount points for that device. A process that still has an
open file or working directory on the filesystem remains an error, so this
does not force an active user off the partition. Two Android tests cover
successful cleanup with and without a local mount, and retain the original
EBUSY result when a private runtime is actively using the filesystem.
All eight wipe UI regression tests and Android Clippy also passed.

On the MIX Fold 2, the v16 recovery completed the original factory-reset
flow: the log ended with `erasing ended: done`, metadata was mounted as a
fresh ext4 filesystem and contained Skyward's settings file, and all vendor
namespace copies were gone. The owner then confirmed that the cleared system
entered Android's initial setup. Data and metadata were cleared by this test;
the previous metadata image was saved before the test.

The v16 image is 104,857,600 bytes, SHA-256
`3a675bb6a6dc3f5f3541970d8d6469f3afb5108644a51473a8a83d05b35079b2`.

## Official OS1 hardware services

On official OS1.0.15.0.ULRCNXM (Android 14), WLAN opened no supplicant
control socket, Data could not find KeyMint, and slot queries could not
reach the boot HAL. These shared a discovery failure: OS1 carries
`hwservicemanager` in `/system/bin`, while newer builds put it in
system_ext. The service plan read only system_ext, vendor and odm, so it
could not construct the HIDL-directory step and started none of the key
or boot services. The supplicant also waited for that directory.

Service discovery now includes system, with its system-as-root tree at
`/mnt/system/system`. Credential-HAL filename fallback excludes the
framework client `gatekeeperd`. The described-file extractor includes the
same system partition and normalizes the system-as-root prefix. A new
fixture covers a system-resident hardware service directory together with
gatekeeperd and a vendor Gatekeeper HAL that lacks an init interface
declaration. All 17 parser/naming unit tests and 11 multi-phone discovery
tests passed on Android; Android Clippy with warnings denied and the
independent image checks passed too.

The v18 image was written to the active `recovery_a`, read back, and booted
against OS1. KeyMint, Gatekeeper, the system-resident hwservicemanager and
the boot HAL all registered. Slot control returned current/next A, both
slots bootable and successful, and no pending snapshot update. WLAN was
ready to scan in 121 ms after background preparation; the first scan
returned three networks. Data mounted as f2fs and user 0 went from locked
to unlocked. The owner confirmed WLAN Internet access, Data decryption,
internal-storage browsing, the slot page, repeated folding and touch.
No Android installation, Data clear or slot switch was performed for this
fix; the other recovery and both slots' boot/vendor_boot/dtbo were unchanged.

OS1 also has a different OTA query revision from its public OS name:
`ro.mi.os.version.incremental` says `OS1.0.15.0.ULRCNXM`, while
`ro.build.version.incremental` says `V816.0.15.0.ULRCNXM`. On 2026-11-16,
otherwise identical official queries using the public name returned no ROM;
using the installed build revision returned `OS1.0.16.0.ULRCNXM`.
Xiaomi queries now read the actual incremental build property; displayed
OS names and the Oplus provider retain their existing properties. The
regression fixture covers OS1, OS3 and MIUI 14 request identities; all
14 Xiaomi/Oplus protocol tests passed on Android.

The integrated v19 was written to `recovery_a` and read back. Its live
query named `V816.0.15.0.ULRCNXM` and returned the official
`OS1.0.16.0.ULRCNXM` full package, 5,953,001,418 bytes. The owner confirmed
that version on the update page and repeated Data, slot information,
folding/display and touch checks successfully. No system update was
downloaded or installed during this query validation.

The final v19 image is 104,857,600 bytes, SHA-256
`73ddc4b94d5dbd2a1d9efd7f9e89f45c95542c5293b9d4dcfee9f8fa29b7d758`.

## Installing while an APEX holds a partition open

Installing the official `V14.0.14.0.TLRCNXM` full package from OS1 stopped
before the update engine ran: `unmount /mnt/system_ext: Device or resource
busy`. No process directly mapped a file on that partition, but loop0
still held `/mnt/system_ext/apex/com.android.vndk.v31.apex` as the backing
file of the mounted VNDK bundle. The process-only pinning pass missed that
kernel-held reference and classified system_ext as unused.

The pinning pass now includes the files held by attached loop devices.
Containers on the installed slot's mounted partitions are locked in
memory whole and their source mount points join the existing busy set.
The normal release path can then detach those mounts while their old
files and devices remain usable; genuinely unused partitions still use
strict unmounting and namespace-copy cleanup.

Two Android-side tests passed using RAM-backed files and private mount
namespaces. One reproduces EBUSY with only an APEX-like loop view, pins its
container, detaches the source filesystem, overwrites the simulated old
partition and reads the original payload through the loop. The other
verifies that an unused filesystem still unmounts normally. The real boot
partition hashes and init mount namespace were unchanged by these tests.
Android Clippy with warnings denied and the independent image checks
also passed.

The owner then retried the same 6,008,392,048-byte official MIUI 14 package.
The runtime and VNDK APEX containers were both pinned, system_ext left the
mount tree, all 4,105 payload operations completed, and signature checking,
filesystem verification and postinstall finished successfully. The engine
reported `Update successfully applied`; Recovery reported
`Installed { slot: B, wipe: false }`. Skyward was preserved in recovery_b,
and both recovery partitions read back as v20. The owner confirmed the
completed result page and normal folding/display and touch. The phone
remained in the A-slot Recovery result page with B selected for the next
system start; starting the installed MIUI 14 was not tested in this round.

The postinstall log also recorded an automatic metadata remount warning:
the filesystem-probing mount tried erofs and received EBUSY. An explicit
read-only ext4 mount restored `/metadata` successfully, and the boot HAL
reported no pending update. This was recorded separately from the fixed
system_ext failure; metadata was not formatted.

The v20 image is 104,857,600 bytes, SHA-256
`563cf5c9446645df85a7fffc27c4a6ae66bfd7143b55f3ab88464b156d59b974`.

## Factory reset immediately after installation

After the MIUI 14 installation above, a factory reset in the same Recovery
session failed before erasing anything: `Data failed: the phone's table
has no /data`. Data was still mounted on dm-7, but the installer had taken
the old slot's vendor and odm filesystems out of Recovery's mount tree.
The reset planner read their fstab files again and found no entries.
Changing the next boot slot did not restore these files. The device's
userdata and metadata entries are shared partitions, without `slotselect`.

Before an update or install script releases the installed system, the
installer now keeps its complete parsed fstab in memory. Data opening,
filesystem operations and wipe planning use this copy for the rest of
that Recovery session. All filesystem alternatives, options, encryption
flags and vendor/odm ordering are preserved. Repeated installation does
not replace the booted system's policy with that of the next selected
slot. A new Recovery process reads the system it starts with; ordinary
startup reads do not freeze an incomplete table before vendor is mounted.
The original confirmation and wipe behavior remain in place.

All eight fstab tests passed on the phone, including three new regressions
for removing both source files, an early read before the files exist, and
repeated preservation followed by a new Recovery's read. These tests left
the real boot partitions and init mount namespace unchanged. Android
Clippy with warnings denied and independent image checks passed.

Booting Recovery on the installed MIUI 14 also exposed a vendor-runtime
dependency issue: Gatekeeper's HAL was loaded in the system namespace,
which did not search vendor for `libkeymasterdeviceutils.so`. The private
runtime's system namespace now searches vendor/odm after the installed
framework directories. Framework libraries retain their search priority.
Gatekeeper, KeyMint and boot control then started normally on a fresh v22
boot, without modifying the generated runtime configuration by hand.
The older firmware still refused the existing Data key with error -38;
this was not treated as a missing partition or bypassed. The owner agreed
to reinstall through sideload and erase the old Data.

The same 6,008,392,048-byte MIUI 14 full package was installed from the
B-slot v22 Recovery to A through ADB sideload. All 4,105 payload operations,
signature checking, filesystem verification and postinstall succeeded.
Both source fstab files were confirmed absent from Recovery's mount tree.
The owner then performed the original factory-reset flow without rebooting:
`erasing ended: done`. Boot ID and Recovery PID were identical before and
after the reset. Data and shared storage were no longer mounted, metadata
was newly formatted as ext4 and had no old metadata-encryption key, and
all eight boot/recovery partition hashes were unchanged by the reset.
No manual metadata remount or fstab-file restoration was used between the
install and reset. The previously recorded automatic metadata remount
warning still occurred; it did not prevent this reset. The owner then
confirmed that the new MIUI 14 booted into Android setup on A.

Both recovery partitions read back as v22. Its image is 104,857,600 bytes,
SHA-256
`14edb6b3061b5326f84bbe51ccf58a9052b578a7aaec8d2715f47fdf14fb0ecd`.

## MIUI 14 identification and encrypted Data

After the official MIUI 14 installation and setup, Recovery displayed only
Android 13 and refused Data with `Android too old`. The installed system
has no HyperOS display-version property. Its complete version is
`ro.build.version.incremental=V14.0.14.0.TLRCNXM`, and its MIUI properties
identify major version 14. Device information now falls back to that build
revision for MIUI, using the installed system's properties rather than
Recovery's inherited build. The systems page also names it `MIUI 14`.
HyperOS keeps its preferred public version when both property sets exist.

The key reader previously rejected any vold key directory containing a
`stretching` file. Android 13's layout-1 metadata key on this phone has
`stretching=nopassword`, which is a supported AOSP storage format rather
than an unknown key layout. The reader now handles `nopassword` and `none`
with their original application-ID binding rules. A directory without the
file keeps the modern behavior. Hardware authentication and software GCM
authentication remain in place; unsupported stretching such as `scrypt`
and unknown layout versions still return explicit errors. No keys are
rewritten and no Data format is needed for this fix.

Android-native tests passed: 10 key-storage tests, one installed-version
regression, and 37 systems tests, with two unrelated platform-dependent
systems tests ignored. Coverage includes the Android 13 hardware-key
binding, software keys with and without the legacy marker, rejection of
the wrong secret or changed binding, MIUI 14 naming, and HyperOS priority.
The tests left boot partitions, init mounts and metadata key files
unchanged. Android Clippy with warnings denied and independent image
checks passed.

The owner confirmed the full MIUI version, successful credential-based
Data decryption and visible internal storage on a fresh v23 boot. The
Recovery log records user 0 changing from closed to open, with Data
mounted as f2fs and shared storage exposed at `/sdcard`. A live update
query used the installed MIUI revision and returned `V14.0.15.0.TLRCNXM`;
no package was installed during this check. Both recovery partitions read
back as v23; boot, vendor_boot and dtbo were unchanged in both slots.
The phone remains in A-slot Skyward. This round did not clear Data.

The v23 image is 104,857,600 bytes, SHA-256
`49e81d39c0d9a89f2b57e69954515dd0bcabf04de11a4602a5c6239c6e8708cc`.

## MIUI 13 downgrade compatibility and settings after a reset

On official `V13.1.21.0.SLRCNXM` / Android 12, the VNDK 31 bundle is named
`/mnt/system/system/apex/com.android.vndk.current.apex`. Its protobuf
manifest names it `com.android.vndk.v31`. The previous lookup only tried
versioned filenames. It missed this bundle, prepared no private vendor
runtime, and qseecomd repeatedly exited before the key, credential and
boot-control services could start. The fold client also had no runtime,
leaving Recovery on the cover display. Waiting for the missing listener
property added 20 seconds to startup. WLAN's supplicant shared the missing
runtime dependency.

Bundle lookup now retains the numbered-file search first, then recognizes
the older `current` filename by its manifest name. It mounts the matching
bundle at the normal versioned APEX path and uses the existing private
runtime for vendor services, the fold client and the WLAN supplicant. A
current bundle belonging to another VNDK version is not substituted.
The bundle and the installed partitions stay read-only.

Recovery settings live on metadata, so formatting that partition removes
their file. The existing UI restores its in-memory settings after a wipe,
but its two-second write debounce exceeded the result page's 1.5-second
automatic restart. Settings restoration after metadata is made afresh is
now due on the first safe write, before that restart. Ordinary slider
changes keep their debounce; a failed write still waits for a change
rather than retrying on every frame. This uses the existing wipe and
result pages without another confirmation step.

All 17 related Android-native tests passed: three VNDK lookup cases,
nine settings cases, and five network-time cases. Coverage includes
restoration before automatic restart, failed-write behavior, SNTP packet
handling, the 2036 timestamp wrap and a local UDP exchange. The tests left
the real boot partitions, init
mount namespace, metadata keys and settings unchanged. Android Clippy
with warnings denied and independent image checks passed.

The first v24 boot on B started KeyMint, Gatekeeper, the hardware service
directory, boot control and the fold sensor. It selected the unfolded
inner panel automatically. The kernel-timestamp interval to the first
Recovery frame fell from 24.33 seconds on the previous boot to 6.44 seconds;
these are log timestamps, not optical boot-time measurements. The live
slot query returned current/next B, both slots bootable and successful,
and no pending update. Existing Recovery settings and metadata keys were
unchanged across this boot.

The owner confirmed faster startup, inner/cover display switching, touch,
Data decryption, internal storage, WLAN Internet access, slot information
and settings on v24. The update query then exposed a separate time issue:
persist's RTC offset put Recovery on 2022-09-20, so the official HTTPS
server's current certificate was not yet valid according to that clock.
After obtaining a WLAN address, v25 asks network time in the background
before completing the reachability check. It uses the network's DNS and
SNTP replies matched to the requested endpoint and random request stamp.
Unsynchronized, unrelated or unusable replies are refused. The running
clock is corrected without writing the PMIC RTC or Android's persist
partition, and HTTPS retains its certificate and date verification.

On the integrated v25 boot, the first frame completed at 6.37 seconds.
After WLAN connected, the network clock was set successfully and the
official query returned `V13.1.25.0.SLRCNXM`, 5,842,347,190 bytes. The owner
confirmed that version, settings retained after reboot, correct display
and folding, successful Data decryption and visible internal storage.
The settings JSON also matched its pre-reboot contents, and the log
recorded user 0 unlocked. Both recovery partitions read back as v25;
both slots' boot/vendor_boot/dtbo and the metadata key files were unchanged.
The phone remains in B-slot Recovery. No Android install, slot switch or
Data/metadata clear was performed for this fix. Settings restoration after
a format was covered by the regression tests without another real wipe.

The integrated v25 image is 104,857,600 bytes, SHA-256
`fb1b1e2a346bc00c1d604d50c6852bca96de65a538e89b8a003b6a1b863afda7`.

## MIUI 13 Data backup and memory extension

On official MIUI 13 `V13.1.21.0.SLRCNXM` / Android 12, Data opened and
user 0 unlocked, but the archive stopped at `/data/extm` with
`Required key not available (os error 126)`. The installed ROM enabled
MIUI memory extension, and that directory held a 3 GiB runtime file
whose separate key was not available in Recovery.

The v26 device configuration excludes only the root `extm` directory
through the existing Data backup policy. The same policy leaves it
untouched during restore. Ordinary application folders named `extm`,
similarly prefixed paths, and other unreadable application data retain
their existing behavior. The backup format, shared key reference and
original UI workflow are unchanged.

A regression failed with the preceding device configuration, then passed
with this exclusion. It covers measurement, packing, restoration into an
existing directory and into an empty directory, and similarly named app
files. The four backup-worker tests and nine archive/store/manifest tests
passed natively in Recovery RAM. The tests did not change real partitions,
init's mounts, Recovery settings or metadata key files. Android Clippy
with warnings denied and the independent image checks passed.

The v26 candidate completed the backup, but a subsequent read-only audit
failed at the PAX metadata of `app-ephemeral`. The tar dependency splits
PAX records at newlines, which may occur inside a raw binary extended
attribute. A synthetic archive reproduced that parsing failure without
touching real Data. v27 keeps existing `SCHILY.xattr` records where the
reader supports them, and encodes attributes with newline values or
newline/equals in names as `SKYWARD.xattr-hex` records. Restoration decodes
the original name and bytes. The test covers legacy text, NUL, non-UTF-8,
newlines, all 256 byte values and the affected attribute names. The
manifest remains format 1; the original product workflow is unchanged.

The owner repeated a Data-only backup through the original UI on v27.
The format-1 encrypted-Data manifest records 3,197,935,104 uncompressed
archive bytes in three volumes, saved at
`/sdcard/Skyward/backups/20220920-190644`. The same store reader and tar
library used by restoration read all 10,464 entries, PAX records and
file contents successfully; the complete digest matched the manifest.
The archive includes ordinary application data and contains no `extm`
or internal-storage entries. The owner confirmed completed backup,
normal internal storage, folding and touch.

Four backup-worker tests and ten archive/store/manifest tests passed
natively on the final build, including both regressions that failed
before their fixes. Clippy with warnings denied and independent image
checks passed. Both recovery partitions read back as v27; both slots'
boot/vendor_boot/dtbo, metadata key files and Recovery settings were
unchanged. No Android install, slot switch or Data/metadata clear was
performed. The earlier v26 candidate backup is preserved outside the
normal backup list at
`/sdcard/Skyward/diagnostics/backup-v26-pax-diagnostic-20220920-184021`.
The backup folder dates follow the phone clock without WLAN time
synchronization on these boots; the host validation date is 2026-11-16.
The automated backup audit did not restore Data. The owner's subsequent
MIUI 13 partition-restoration results are recorded below.

The final v27 image is 104,857,600 bytes, SHA-256
`9a38a2bde9a64998de2dfcfee052c2ddac6bdfd5805b29da121a5ab8c6937ec8`.

## MIUI 13 owner-confirmed partition restoration

On 2026-11-16 (Asia/Shanghai), following the v27 backup fix on official
MIUI 13 `V13.1.21.0.SLRCNXM` / Android 12, the owner reported:
“MIUI13 启动与Data分区恢复通过”. Boot-partition restoration and Data-partition
restoration are therefore recorded as passed on this device and ROM,
against the v27 image identified above.

The source of these two results is the owner's real-device confirmation.
The earlier automated backup audit remains a read-only archive check;
this follow-up did not repeat restoration or collect partition readback,
sample-file comparisons or a separate post-restore Android boot log.
These results do not cover System/EFS restoration, kernel installation,
other official ROM branches or the remaining checklist cases.

## MIUI 13 owner-confirmed clearing and credential types

On 2026-11-16 (Asia/Shanghai), the owner further reported:
“MIUI13 清除功能、各密码种类通过”. Recovery clearing functions and Data
decryption across the supported credential types are recorded as passed
on the current v27 / official `V13.1.21.0.SLRCNXM` / Android 12 baseline.
The evidence source is the owner's real-device confirmation; no device
clearing or credential testing was repeated while recording this result.

The report signs off these feature groups. It does not supply per-option
clearing logs, an install-then-reset sequence, post-clear settings timing
or incorrect-credential cases. Those checklist details and other official
ROM branches retain their separate status.

## Build and validation status

Build on Windows with the local base, Android NDK and LZ4:

```powershell
.\scripts\build-zizhan.ps1 -Base 'path\to\recovery.img' -Ndk 'path\to\android-ndk' -Lz4 'path\to\lz4.exe'
```

The script creates `target/zizhan/skyward-zizhan-experimental.img` and its
SHA-256 file. It does not write to a phone. This local base is not part of the
CI's automatic official-package download matrix.

Configuration/module/packer tests and Android workspace Clippy for all targets
with warnings denied have passed. Android-side tests cover rotation, framebuffer
padding and colour order, page and display-power state across resizing,
cancelled confirmation gestures, the existing power-key and idle behavior,
the QMI structure ABI, and decoding a real sensor sample. Independent
image checks verify the AVB hash, preserved stock files, embedded configuration
and executable, static system-init and original backups.

Recovery ADB, logical partition mounting, KeyMint and Gatekeeper startup are
working. The owner has confirmed both calibrated displays and touch panels,
vibration, unencrypted internal storage, MTP, WLAN, the Xiaomi update query,
automatic panel switching with page retention, screen-off preservation, and a
single direct closed-device boot without the old delay. Clearing the inactive
inner panel's boot splash has also passed the owner's final integrated check.

The final startup handoff change passed separate unfolded and closed boots.
The owner reported that both Logo-to-UI black gaps were now very short, and
confirmed direction, touch, no residual inner Logo, repeated panel switching,
page retention and screen-off/power-key behavior again. Kernel timestamps from
these two boots measured 280.15 ms on the inner display and 285.00 ms on the
cover from the UI's display-takeover marker to its first-frame-complete marker.
These are software handoff intervals, not optical measurements of black-screen
duration. Both boots started the recovery service once. Final partition and
running-binary SHA-256 checks matched the built image, and the UI capacity hint
was back at zero outside the large About page.

Official OS3.0.9.0 full-package installation through ADB sideload and
subsequent Android boot have been validated as described above. Encrypted
Data decryption is validated on official OS1.0.15.0.ULRCNXM,
MIUI 14 V14.0.14.0.TLRCNXM and MIUI 13 V13.1.21.0.SLRCNXM.
Encrypted Data backup on MIUI 13 is validated above; the owner subsequently
confirmed boot-partition and Data-partition restoration on that ROM.
The owner also confirmed MIUI 13 clearing functions and Data decryption
across credential types on v27.
Factory reset
including Data and metadata clear is validated with v16. OTA download,
System/EFS restoration, kernel installation and explicit filesystem
conversion have not been validated on MIX Fold 2.
Results from the separate OnePlus 13 port do not validate these functions
on this device.
