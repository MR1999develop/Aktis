# Istruzioni di sviluppo di Aktis

- Rispondere in italiano; interfaccia, codice, commenti, documenti e commit
  in inglese.
- Linguaggio: Rust + egui; calcolo su GPU con wgpu (WGSL) e un motore CPU di
  riferimento: ogni strumento ha la versione CPU e il kernel GPU, che danno lo
  stesso risultato.
- Applicare le skill di progettazione prima di scrivere o rivedere codice:
  - sempre: `rust-egui-desktop-design`, `software-design-core`,
    `image-processing-design`;
  - interfaccia: `ui-design-core`;
  - GPU, kernel WGSL e viewer: `gpu-design`;
  - catalogo SQLite, ricette e salvataggi: `durable-data-design`;
  - file non fidati (RAW, DNG, profili), download di modelli e del runtime
    ONNX, server MCP, rilascio: `security-design`;
  - API di Windows, processi e DLL: `windows-system-design`.
- Controlli: `scripts\check.bat` (formato, clippy, test, build di rilascio;
  `-Quick` salta la build di rilascio). Su un PC nuovo prima
  `scripts\setup-windows.bat`.
- Alla fine di ogni blocco di lavoro aggiornare `docs/progress.md` (stato) e
  `TIDY.md` (cose rimandate).
- Licenza `GPL-3.0-only`, copyright e autori **MR**; nessun dato personale
  nei file né negli eseguibili: gli script compilano passando da
  `scripts\build-env.ps1`, che toglie i percorsi locali.
