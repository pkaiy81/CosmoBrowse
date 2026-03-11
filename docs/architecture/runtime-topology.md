# Runtime topology

CosmoBrowse fixes the Browser/Renderer boundary with the following two-process model.

```text
+---------------------------+          Tauri invoke IPC (JSON DTO)
| Browser Process           | <-----------------------------------> +--------------------------+
| - tauri command handlers  |                                       | Renderer Process (WebView)|
| - session/history control |                                       | - frame tree drawing      |
| - network loader          |                                       | - diagnostics panel       |
| - crash dump writer       |                                       | - user input              |
+---------------------------+                                       +--------------------------+
            |                                                                    |
            | writes minimal crash dump                                           | reads BrowserPageDto
            v                                                                    v
  /tmp/cosmobrowse-crash-report.json                                   network log / DOM snapshot / console
```

## Boundary rules
- The Browser process returns `BrowserPageDto` as the single IPC DTO.
- The Renderer depends only on the DTO and does not depend on `SabaApp` internal structures.
- On crash, the panic hook emits a minimal JSON dump and reproduction steps.
