# YoutubeInRustWeb (MusicLowCost)

Reproductor nativo de YouTube Music en Rust (Slint + rodio). La meta de diseño es gastar
poca memoria: cualquier cambio se mide contra eso. Detalles y estructura en `README.md`.

## Contexto del código: grafo + vectores

Heredado de KodBrowser (y este de BlackImperial). Antes de barrer el repositorio con búsquedas de texto, se consulta el índice local (`.codegraph/`, no se versiona; requiere Node ≥ 22.5 y `npm install` una vez):

- **Estructural, exacto** —«¿quién llama a esto?», «¿qué rompe este cambio?»—: `npx codegraph callers <símbolo>`, `npx codegraph callees <símbolo>`, `npx codegraph impact <símbolo>`.
- **Por concepto** —«¿dónde se resuelve el audio con yt-dlp?», «cómo se sincroniza el reloj del Jam»—: `node scripts/grafo/buscar.mjs "consulta" [--k 8] [--solo src/jam/] [--json]`. Fusiona similitud vectorial (multilingual-e5-small) con BM25 y devuelve cada resultado con quién lo llama y a quién llama.

**La documentación de diseño solo la cubre la capa vectorial**: la cabecera `//!` de cada módulo y las secciones de `README.md` —donde vive el porqué de cada decisión— se encuentran con `buscar.mjs`, no con `codegraph callers`. Todo es incremental: el grafo se sincroniza al editar (hook en `D:\Portafolio\.claude\settings.local.json`) y los vectores en la siguiente búsqueda. `ui/app.slint` no lo indexa CodeGraph (no conoce Slint): se lee directo. `vendor/` (crates de terceros con parches marcados `YoutubeInRustWeb`) y `version-webview/` quedan fuera del índice. `package.json` y `node_modules/` son solo de esta herramienta: la app no depende de Node.

## Reglas del proyecto

- La interfaz está en español; los comentarios del código también (sin acentos en el código Rust, como el resto).
- `NOTAS-PARA-MAÑANA.md` es privado (está en `.gitignore`).
- Pruebas que necesitan red están marcadas `#[ignore]`: `cargo test -- --ignored` (Jam por internet, streaming real, listas y Me gusta con la sesión guardada).
