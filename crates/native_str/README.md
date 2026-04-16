# native_str

A platform-native string type for lossless, zero-copy IPC.

`NativeStr` is a `#[repr(transparent)]` newtype over `[u8]` that represents OS strings in their native encoding:

- **Unix**: raw bytes (same as `OsStr`)
- **Windows**: raw wide character bytes (from `&[u16]`, stored as `&[u8]` for uniform handling)

## Why not `OsStr`?

`OsStr` requires valid UTF-8 for serialization. `NativeStr` can be serialized/deserialized losslessly regardless of encoding, with zero-copy support via wincode's `SchemaRead`.

## Limitations

**Not portable across platforms.** The binary representation of a `NativeStr` is platform-specific — Unix uses raw bytes while Windows uses wide character pairs. Deserializing a `NativeStr` that was serialized on a different platform leads to unspecified behavior (garbage data), but is not unsafe.

This type is designed for same-platform IPC (e.g., shared memory between a parent process and its children), not for cross-platform data exchange or persistent storage. For portable paths, use UTF-8 strings instead.

## Usage

```rust
use native_str::NativeStr;

// Unix: construct from bytes
#[cfg(unix)]
let s: &NativeStr = NativeStr::from_bytes(b"/tmp/foo");

// Windows: construct from wide chars
#[cfg(windows)]
let s: &NativeStr = NativeStr::from_wide(&[0x0048, 0x0069]); // "Hi"

// Convert back to OsStr/OsString
let os = s.to_cow_os_str();
```
