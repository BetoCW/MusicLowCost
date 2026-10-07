# Línea base de rendimiento — v0.5.0 (2026-10-06)

Medido con `scripts/bench.ps1` sobre el exe de release, más la instrumentación de `src/bench.rs` y del renderer parchado (`vendor/i-slint-backend-winit/renderer/sw.rs`). La instrumentación solo se activa con `YIR_BENCH_FILE`.

**Equipo:** Ryzen 7 5700G (gráficos integrados Radeon), pantalla de **1920×1080 al 100 %** (96 DPI), Windows 11, SSD.
**Datos:** carpeta aparte (`YIR_DATA_DIR`), sin sesión iniciada; Inicio carga el top de México (20 canciones y sus tarjetas).
**Repeticiones:** 5 corridas a 1100×720 (tamaño por defecto) y 3 por tamaño a 1920×1040 y 2560×1400. Las cifras son medianas.

## Qué mide cada cosa

| Métrica | Cómo |
|---|---|
| Primer frame | Desde `Start-Process` hasta que termina el primer `present()` |
| Inicio con contenido | Desde el lanzamiento hasta que Inicio tiene filas, y luego todas sus portadas |
| Frame | `render()` + `present()` del renderer por software (repinta la ventana completa siempre) |
| Scroll | 2000 canciones de prueba en la Cola, una muesca de rueda de 60 px cada 16 ms durante 5 s (`dispatch_event`, la misma vía que el ratón) |
| Clic → frame | `navigate("queue")` → fin del primer frame con la página nueva |
| RAM | `WorkingSet64` / `PrivateMemorySize64` cada 200 ms (lo que muestra el Administrador de tareas / bytes privados) |

## Resultados

| Métrica | 1100×720 | 1920×1040 | 2560×1400 |
|---|---|---|---|
| Primer frame, exe ya conocido por Windows | 72 ms | 71 ms | 75 ms |
| Primer frame, **primera ejecución de un exe recién compilado** | 2 520–2 590 ms | | |
| Inicio con filas (red) | 1 010 ms | — | — |
| Inicio con todas las portadas | 1 280 ms | — | — |
| Scroll: fps | 60,4 | 59,6 | 58,6 |
| Scroll: frame p50 / p95 / máx | 2,3 / 2,8 / 3,3 ms | 4,0 / 5,1 / 5,8 ms | 5,8 / 7,2 / 7,8 ms |
| Clic → frame | 20,5 ms | 22,7 ms | 16,3 ms |
| Reposo sin música: frames/s | 0,2 | — | — |
| Reproduciendo: frames/s | 8 | 8 | 8 |
| Reproduciendo: frame p50 | 2,0 ms | 3,6 ms | 5,0 ms |
| Clic en canción → suena | 2 240 ms | 2 290 ms | 2 210 ms |
| RAM reposo, WS / privada | 36,6 / 15,8 MB | — | — |
| RAM scroll con 2000 filas, WS / privada | 37,6 / 16,2 MB | 42,4 / 20,8 MB | 49,0 / 27,6 MB |
| RAM reproduciendo, WS / privada | 47,0 / 25,6 MB | 52,2 / 30,5 MB | 58,4 / 36,7 MB |
| Decodificar una portada (p50) | 0,09 ms | | |
| Pico de `invoke_from_event_loop` / s | ~90–100 (al llegar las portadas) | | |
| Pico de `set_row_data` / s | ~90 | | |

Notas sobre la tabla:
- El scroll llega a 60 fps porque ese es el ritmo de la rueda simulada. El margen real se ve en el frame: 7,2 ms de p95 a 2560×1400 deja ~9 ms libres por frame.
- En "Clic → frame", la ida y vuelta al hilo de trabajo tarda ~2 ms. El resto es esperar al siguiente frame más dibujarlo (6–9 ms el de cambio de página).
- Reproduciendo hay 8 frames/s porque el ticker de 250 ms produce **dos** frames por tick, separados ~16 ms.
- "Clic en canción → suena" es casi todo red: yt-dlp resuelve el audio y luego se baja el primer tramo.

## Binario

| | Tamaño |
|---|---|
| Exe | 19,6 MiB (20 037 KB) |
| Instalador | 9,4 MB |
| `.text` (código) | 11,9 MiB |
| `.rdata` (datos de solo lectura) | 6,7 MiB |

Crates más pesados en `.text` (`cargo bloat --crates`, que es aproximado): std 2,0 MiB · la app 1,1 MiB · rustypipe 506 KiB · i_slint_core 409 KiB · serde 396 KiB · tokio 371 KiB · tiny_skia 326 KiB · rustls 319 KiB · noq_proto 309 KiB + iroh 305 KiB (Jam) · h2 305 KiB · skrifa 294 KiB · usvg 274 KiB · regex 269 KiB.

**La pieza más grande no es código, son datos.** Los diccionarios ICU de segmentación de texto (chino/japonés, tailandés/lao/jemer/birmano, más un modelo LSTM) suman **~4,1 MB** en `.rdata`. Los trae `i-slint-core`, que pide `parley` con `complex-scripts` sin poder desactivarlo, y solo sirven para cortar líneas por palabra en esos idiomas.

## Lo que no se midió

- **WPR / CPU por función:** no se grabó. El costo por frame ya sale de la instrumentación (2–7 ms) y no hay un cuello de botella que justifique perfilar por función.
- **Arranque en frío real** (después de reiniciar, con la caché de disco vacía): no se midió. Las cifras de "primer frame" son con caché caliente.
- **Comparación con Spotifast:** no se midió.

## Conclusiones para el plan v0.6

1. **El renderer por software no es el cuello de botella.** Repinta la ventana completa a 2560×1400 en 7,2 ms (p95). Pasar a GPU (femtovg) sumaría memoria de driver y DLLs (decenas de MB de working set) sin ganancia medible, y va contra la meta del proyecto. → **La fase GPU se descarta**, salvo que una medición futura la justifique.
2. **El repintado parcial ahorra poco:** 2–5 ms por frame, unas 8 veces por segundo mientras suena música. Lo barato y seguro es quitar el **frame duplicado** de cada tick y bajar el tick cuando no se ve la letra.
3. **Arranque:** 70 ms hasta el primer frame ya cumple "< 250 ms". Los 2,5 s solo pasan la primera vez que corre un exe nuevo. Lo más probable es el escaneo del antivirus (Defender) sobre el binario recién escrito, aunque no está verificado; lo vería cada usuario tras instalar o actualizar.
4. **Tamaño:** el mayor recorte posible son los 4,1 MB de ICU. Requiere parchar `parley` o `i-slint-core` en `vendor/`, a cambio de cortar líneas por carácter (no por palabra) en CJK y tailandés. Lo demás del plan de recorte (tokio sin `full`, reqwest con rustls, symphonia solo AAC/MP4, `opt-level="s"`, LTO, `strip`) **ya está aplicado**.
5. **Lo que el usuario sí percibe:** los ~2,2 s desde el clic hasta que suena (red y yt-dlp) y ~1 s hasta que Inicio tiene contenido (red). Ahí está la ventaja frente a una app web, no en los fps.
