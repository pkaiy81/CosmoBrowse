# Runtime topology

CosmoBrowse fixes the Browser/Renderer boundary with the following two-process model.

> Diagram source: `docs/architecture/mermaid/runtime-topology.mmd`

```mermaid
flowchart LR
  BP["Browser Process\n- adapter_native\n- session/history control\n- network loader\n- crash dump writer"]
  RP["Renderer Process (WebView)\n- frame tree drawing\n- diagnostics panel\n- user input"]
  DUMP["/tmp/cosmobrowse-crash-report.json"]
  LOG["network log / DOM snapshot / console"]

  BP <-->|"Generic IPC\n(JSON message schema)"| RP
  BP -->|"writes minimal crash dump"| DUMP
  RP -->|"reads IpcResponse::Page(BrowserPageDto)"| LOG
```

## Boundary rules
- Browser/Renderer communication uses framework-neutral schema (`IpcRequest`/`IpcResponse`).
- The Renderer depends only on DTOs (`BrowserPageDto`, `NavigationState`, `TabSummary` etc.) and never on `saba_app` internal structures.
- `dispatch_ipc` is the default command entrypoint, while per-command Tauri handlers are compatibility mode.
- On crash, the panic hook emits a minimal JSON dump and reproduction steps.
