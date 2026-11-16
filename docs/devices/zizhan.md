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
Full Data restoration has not been performed on the reference phone.

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

Encrypted Data decryption, OTA download/install, boot backup/restore, kernel
installation, Data format, and Data restoration have not been validated on MIX
Fold 2. Results from the separate OnePlus 13 port do not validate these functions
on this device.
