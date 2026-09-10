# `httpPatchUSB` and the host-mediated HTTP path

This document records the static analysis of the PRS-350 firmware module
`/opt/sony/ebook/application/httpPatchUSB.xsb`, the native
`/opt/sony/ebook/application/switcher.so` extension, and the matching
historical host-side `DeviceAccessor.dll`. No Windows library or device write
operation was required. The conclusions below are therefore based on retained
bytecode, ELF symbols/disassembly, PE exports/disassembly, and strings.

## Conclusion

`httpPatchUSB` is a Kinoma XS compatibility layer. It makes the application’s
normal `FskHTTP.Client` API transport requests through Sony’s proprietary
USB/SCSI command service instead of sending them directly over a network
socket.

It is not a USB driver, a USB networking gadget, or a standalone HTTP server.
The connected reader exposes USB Mass Storage, and the HTTP path is an
application-level request/response tunnel carried by Sony’s mass-storage
protocol.

The intended sequence is:

```text
ebook/DRM code
    |
    | normal FskHTTP.Client request
    v
storage VM: httpPatchUSB.xsb
    |
    | HTTPRequest message + System.Future
    v
switcher VM: native switcher.so
    |
    | Sony USB command 0x40 / 0x41
    v
USB Mass Storage transport
    |
    v
host-side Sony application/library
    |
    | service request and response
    v
reader-side Future completion and FskHTTP callbacks
```

The diagram describes the software contract recovered from the image. It
does not establish which particular Sony desktop application consumes the
request or which remote services it contacts.

## Firmware-side bytecode

`storage.xml` loads both the storage module and the USB HTTP patch into the
storage VM:

```xml
<bytecode href="[applicationPath]storage"/>
<bytecode href="[applicationPath]httpPatchUSB"/>
```

The recovered XS11 bytecode shows the following behavior:

- patches `FskHTTP.Client` defaults and methods;
- stores the URL, request method, request headers, and optional body stream;
- adds `Content-Length` for a streamed POST request;
- supplies the system HTTP User-Agent when one was not provided;
- creates a `System.vm.Reference("switcher")` and a `System.Future`;
- posts `messages.HTTPRequest(client)` to the switcher VM;
- maps the returned status, status code, headers, and body back onto the
  original client; and
- invokes the normal `onHeaders`, `onDataReady`, and `onTransferComplete`
  callbacks.

`close()` only replaces the Future completion callback. The bytecode does not
show a transport-level cancellation operation.

The switcher/storage message vocabulary also includes USB registration,
connection, disconnection, lifecycle, and `fakeUSBSwitch` messages. The
storage VM enters its USB state by calling `USBDispatcher.doRegister()` and
routes `HTTPRequest` messages to the switcher VM.

## Native firmware bridge

`switcher.xml` loads the native switcher extension for the switcher VM. The
stripped ARM ELF retains enough dynamic symbols and strings to identify a
complete command implementation, including:

| Firmware symbol or class | Finding |
|---|---|
| `CPRSUSBCommandManager::CreateCommand` | dispatches command IDs to native command objects |
| `CPRSUSBCommand_GetHttpRequest` | implements command `0x40` |
| `CPRSUSBCommand_SetHttpResponse` | implements command `0x41` |
| `CPRSUSBCommand_GetHttpNeedRegistration` | implements command `0x42` |
| `CPRSUSBCommand_GetMarlinState` | implements command `0x43` |
| `EBookUSBHttpGetHTTPRequest` | native-to-XS request delivery helper |
| `EBookUSBHttpSendHTTPResponse` | native-to-XS response delivery helper |
| `switcherSet_gUSBWatcher` and `switcherSet_gUsbConnectHandler` | connection-state callbacks |
| `USBThreadProc` | monitors `/proc/usbtg/connect` and updates USB state |

The native code imports file, thread, `ioctl`, and device I/O functions. It
does not import the usual TCP client functions `connect`, `send`, or `recv`.
That supports the conclusion that this bridge itself is not performing normal
network HTTP.

The `/proc/usbtg/connect` access is a connection-state/control mechanism. It
is not evidence of a separate USB serial or Ethernet interface.

### Request direction and shape

The native `GetHttpRequest` implementation calls an internal helper with two
input buffers. When no callback is installed, that helper allocates a maximum
`0x3400`-byte result, copies the two NUL-terminated input strings into it, and
returns the result pointer and size. The command operation accepts a `0x200`-
byte first-phase input and later transfers data in chunks capped at `0x1000`
bytes.

The matching host wrapper confirms the two fixed-size fields. Its
`MSC_ebookUsb_getHTTPRequestfromDevice` function:

1. allocates a `0x200`-byte buffer;
2. copies two caller-supplied strings into separate `0x100`-byte slots;
3. zero-pads the slots;
4. sends command `0x40`; and
5. returns two 32-bit result values to its caller.

The two request strings are still not named by the retained host code, but the
wire-level result is now established. Both control headers are two little-
endian 32-bit words:

```text
word 0: status or callback result
word 1: number of body bytes that follow
```

For command `0x40`, the phases are:

```text
host -> device, phase 1: 0x200 bytes (two 0x100-byte string slots)
device -> host, phase 3: 8-byte control header
device -> host, phase 3/2: body, at most 0x1000 bytes per transfer
```

The device sends the body with phase `3` for intermediate chunks and phase
`2` for the final chunk. The host uses the second control word as the body
length and allocates that many bytes. In the native no-callback fallback the
status is zero, the length is `0x3400`, and the body is a zero-filled
`0x3400`-byte allocation containing the two input strings as NUL-terminated
copies. The first word is not explicitly checked by the retained `0x40` host
operation, so its precise error semantics remain less certain than its
position and encoding.

### Response direction and shape

The native `SetHttpResponse` operation reads the first 32-bit word of its
input and uses it as the response-body length. Its phases are:

```text
host -> device, phase 1: 4-byte little-endian body length
host -> device, phase 3: body, at most 0x1000 bytes per transfer
device -> host, phase 3: 8-byte control header
device -> host, phase 3/2: optional returned body, at most 0x1000 bytes per transfer
```

The host sends the body in phase-3 chunks, including the final body chunk. It
then reads the 8-byte control header and explicitly rejects a nonzero first
word. The second word is the returned-body length; if nonzero, the host reads
that body using phase `3` for intermediate chunks and phase `2` for the final
chunk.

The host wrapper `MSC_ebookUsb_sendHTTPResponsetoDevice` sends command `0x41`
with a caller-provided pointer and length and returns two 32-bit result values.
The firmware-side callback helper receives the complete host-supplied body and
can return a separate buffer/status pair. The XS layer then constructs a
response object containing the fields consumed by `httpPatchUSB`: `status`,
`statusCode`, `headers`, and `body`.

In the native no-callback fallback, the device returns status zero and a
zero-filled `0x3400`-byte buffer. This fallback is not a useful HTTP response;
it is evidence that an installed native callback is expected to perform the
actual host-mediated handoff.

### SCSI transport framing

The host DLL carries these operations through Windows
`IOCTL_SCSI_PASS_THROUGH`. The pass-through buffer is initialized with a
10-byte CDB targeting SCSI target ID `1`, LUN `0`, and the following command
fields:

```text
CDB[0] = 0x20
CDB[2] = Sony application command ID (0x40 or 0x41)
CDB[4] = phase (1, 2, or 3)
```

The data direction is selected by the pass-through `DataIn` field. The normal
transfer buffer is at offset `0x50`, and the host-side timeout is `10000 ms`.
This explains why the Linux SCSI-generic client can see the device as a
Mass-Storage target while the HTTP exchange remains outside the standard
SCSI command set.

## Host DLL evidence

The historical Windows-side `DeviceAccessor.dll` retains a direct export set
matching the firmware symbols:

| Host export | Command | Static behavior |
|---|---:|---|
| `MSC_ebookUsb_getHTTPRequestfromDevice` | `0x40` | sends two `0x100`-byte string fields and returns two 32-bit values |
| `MSC_ebookUsb_sendHTTPResponsetoDevice` | `0x41` | sends a caller-provided variable-length response |
| `MSC_ebookUsb_needsRegistration` | `0x42` | sends one `0x100`-byte string and receives a 32-bit result |
| `MSC_ebookUsb_GetMarlinState` | `0x43` | retrieves the device DRM/media state |
| `MSC_ebookUsb_FreeMarlinState` | — | frees the returned state allocation |
| `MSC_ebookUsb_GetDeviceID` | — | retrieves the device identifier |

The same DLL also exports the file operations used by the existing protocol
ledger (`MSC_UsbFile_GetSize`, `MSC_UsbFile_Read`, `MSC_UsbFile_Write`, and
`MSC_UsbFile_Delete`). Its RTTI contains the same `CPRSUSBCommand_*` class
names as `switcher.so`, providing strong evidence that the host and device
implementations belong to the same protocol family.

The retained host binaries import only the USB/file bridge, Windows setup and
UI/runtime libraries, and OLE support. There are no imports from
`ws2_32.dll`, `wininet.dll`, `winhttp.dll`, or `urlmon.dll`, and no socket
functions such as `connect`, `send`, or `recv` appear in the relevant import
sets. Therefore `USBDLL.dll` is a SCSI tunnel, not the component that fetches
the remote URL. The network-side consumer is a separate Sony desktop
component that is not present in the retained artifacts.

The older `ebook_msc.c` utility does not exercise these HTTP functions; it is
useful mainly as confirmation of the surrounding file-service API. The HTTP
exports are present in `DeviceAccessor.dll` even though the legacy sample does
not call them.

## What this rules in and out

Established by static analysis:

- the ebook application can issue HTTP-shaped requests without using its
  ordinary network client path;
- the request is handed from the storage VM to the switcher VM;
- firmware command IDs `0x40` and `0x41` carry the request/response exchange;
- both command control headers are little-endian `(status, length)` pairs;
- command `0x40` uses a `0x200`-byte request and command `0x41` uses a 4-byte
  body-length prefix;
- a matching host-side API was intended to retrieve requests and return
  responses; and
- the mechanism is tied to the USB Mass Storage/SCSI command service.

Not established yet:

- the semantic names of the two `0x100`-byte request fields;
- the identity of the Sony desktop component that services the request;
- the remote DRM/service URLs and authentication sequence;
- whether all PRS-x50 firmware revisions use identical fields; or
- whether the host API can be safely exercised independently of the full Sony
  application stack.

“Arsenal” appears as an internal firmware event/state name. The image does
not provide enough evidence to identify it as a separate external protocol.

## Recommended follow-up

Since the library cannot be run here, the highest-value analysis is static:

1. locate the missing Sony desktop component that calls the HTTP exports;
2. identify the semantic names of the two request-string slots from that
   caller; and
3. if a compatible Sony application is ever available, capture one known
   transaction and compare its `0x40`/`0x41` phases with the static layout.

The current `prsctl` client should remain read-only and should not expose
`0x40` or `0x41` until their framing and safety properties are confirmed.
