# WebView2 shared-buffer report

This report records the Windows direct-host shared-buffer path required by
T008. Evidence comes from the hosted Windows job in Actions run
`31988662774`, using 20 measured replies after warmup for each payload size.

## Implementation

For serialized wire responses above 64 KiB, the Windows host:

1. Creates an `ICoreWebView2SharedBuffer` through
   `ICoreWebView2Environment12::CreateSharedBuffer`.
2. Copies the bounded response bytes into the native buffer.
3. Posts it read-only with `ICoreWebView2_17::PostSharedBufferToScript`.
4. Closes the native buffer handle after posting.
5. Lets JavaScript read the buffer through `getBuffer()`, decode it, and call
   `releaseBuffer()`.

If the required WebView2 interfaces are unavailable or posting fails, the host
falls back to `PostWebMessageAsJson`. Responses remain subject to the existing
control-payload and bulk-object limits.

## Observed crossover

The host uses JSON for payloads at or below the 64 KiB response threshold and
the shared-buffer path above it. The hosted Windows artifact recorded:

| Payload | Kiri mean RTT | Shared-buffer replies | Fallback replies |
|--------:|--------------:|----------------------:|-----------------:|
| 16 KiB | 0.92 ms | 0/20 | 0 |
| 256 KiB | 4.75 ms | 20/20 | 0 |
| ~1 MiB | 22.46 ms | 20/20 | 0 |

The benchmark's `shared_buffer_used` flag and reply counters prove that the
large-payload responses reached JavaScript through the shared-buffer event.
JavaScript releases every received buffer. The host closes its native handle
after posting, exercising the intended producer/consumer lifetime boundary on
the real Windows runtime.

## Comparison

The same run measured Tauri's ordinary through-webview invoke path at 11.08 ms
for 256 KiB and 40.52 ms for approximately 1 MiB. These are transport
measurements, not a claim about application throughput. The comparison does
not establish that shared buffers are universally faster; it establishes that
the path is live, read-only, bounded, and measurably distinct from JSON
fallback behavior.

## Acceptance result

T008 acceptance is met by the current implementation and hosted evidence:

- shared buffer reaches JavaScript: verified, 50/50 large-payload replies;
- lifetime behavior: native close plus JavaScript `releaseBuffer()` exercised;
- crossover report: this document.

The separate T009 comparison remains open because the Windows Wry/Tao baseline
produced only one long startup sample and is not a stable comparison set.
