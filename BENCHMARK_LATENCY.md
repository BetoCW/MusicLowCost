# Latencia de clic a sonido — v0.5.0 (2026-10-06)

Medido con `scripts/bench.ps1 -Runs 10 -Scenarios latency` sobre el exe de release. Cada corrida abre la app de cero con una carpeta de datos limpia, sin sesión ni caché de URLs, y reproduce una canción distinta de Inicio (índices 0–9 del top de México). Todos los tiempos se cuentan en ms desde el clic.

**Equipo:** Ryzen 7 5700G, Windows 11, Node 24 como motor JS de yt-dlp, red doméstica.

## Marcas

| Marca | Dónde | Qué es |
|---|---|---|
| t0 | `src/bench.rs` | clic (`play-track`) |
| t1 | `stream::resolve`, al entrar | empieza la resolución (ya pasó `ensure_tools` y no hubo espera anti-robot de 450 ms) |
| t2 | `stream::resolve`, al salir | yt-dlp devolvió la URL del audio |
| t3 | `stream::start_download` | llegaron los encabezados HTTP del primer rango |
| t4 | `stream::start_download` | llegaron los primeros 384 KB (`FIRST_CHUNK`) |
| t5 | `backend::load_and_play` | `engine.load` armó el decodificador |
| t6 | `audio::Fx::next` | el mezclador pidió la primera muestra |
| t7 | `audio::Fx::next` | primera muestra no silenciosa (depende de la intro de la canción, no de la app) |

## Las 10 reproducciones

| # | t1 | t2 yt-dlp | t3 HTTP | t4 384 KB | t5 decodificador | t6 1.ª muestra | t7 con sonido | UI "sonando" |
|---|---|---|---|---|---|---|---|---|
| 0 | 0,3 | 1 733 | 1 758 | 1 779 | 1 779 | **1 788** | 2 379 | 1 867 |
| 1 | 0,4 | 1 770 | 2 372 | 2 393 | 3 636 | **3 640** | 3 781 | 3 694 |
| 2 | 0,3 | 1 899 | 1 933 | 1 965 | 1 965 | **1 966** | 2 486 | 2 149 |
| 3 | 0,4 | 1 716 | 1 758 | 1 778 | 1 779 | **1 782** | 2 062 | 1 921 |
| 4 | 0,3 | 2 009 | 2 247 | 2 267 | 2 268 | **2 270** | 2 370 | 2 342 |
| 5 | 0,4 | 1 995 | 2 116 | 2 138 | 2 139 | **2 144** | 2 304 | 2 346 |
| 6 | 0,3 | 1 955 | 2 115 | 2 147 | 2 147 | **2 152** | 2 612 | 2 297 |
| 7 | 0,3 | 2 158 | 2 589 | 2 611 | 2 849 | **2 856** | 3 046 | 3 073 |
| 8 | 0,3 | 2 218 | 2 236 | 2 256 | 2 257 | **2 258** | 2 618 | 2 378 |
| 9 | 0,3 | 2 000 | 2 237 | 2 257 | 2 258 | **2 261** | 2 401 | 2 406 |

## Por tramo (mediana · mín–máx)

| Tramo | Mediana | Rango | Parte del total |
|---|---|---|---|
| t0→t1 clic → empieza a resolver | 0,3 | 0,3–0,4 | 0 % |
| **t1→t2 yt-dlp** | **1 975** | 1 715–2 217 | **~89 %** |
| t2→t3 conexión y encabezados de googlevideo | 140 | 18–602 | ~6 % |
| t3→t4 bajar 384 KB | 21 | 20–32 | 1 % |
| t4→t5 armar el decodificador | 0,4 | 0,3–**1 242** | 0 % (2 de 10 fuera de rango) |
| t5→t6 primera muestra | 4 | 1–10 | 0 % |
| **t0→t6 clic → primera muestra** | **2 206** | 1 782–3 640 | 100 % |
| t6→t7 silencio inicial de la canción | 240 | 100–590 | (de la canción) |

## Dentro de yt-dlp

Trazado con `yt-dlp -v`, con las marcas de tiempo tomadas de stderr. Son dos canciones, y la primera se repitió con la caché del desafío ya guardada.

| Paso de yt-dlp | ms |
|---|---|
| Arrancar el intérprete (`yt-dlp --version`) | 370–425 (1 081 la primera vez tras compilar o encender) |
| Descargar la página `watch` | **950–1 020** |
| `visionos player API JSON` | 145–155 |
| `m3u8 information` (formatos HLS, no se usan para audio) | 195–395 |
| Descargar `player.js` | ~95 |
| Resolver el desafío JS con Node (solo en algunos videos) | **~830–970** |

yt-dlp guarda en caché el resultado del desafío (`youtube-sigfuncs`), pero aun así vuelve a lanzar Node en la siguiente resolución del mismo player.

## Qué dicen los números

1. **El 89 % de la espera es yt-dlp.** De esos ~2 s, solo ~10 % es arrancar Python; el resto son 3–4 peticiones a YouTube en serie, más Node cuando el video trae desafío. Todo lo que está del lado de la app (descarga, decodificador, mezclador) suma ~25 ms en el caso típico.
2. **Para bajar de 800 ms hay que sacar a yt-dlp del camino crítico:**
   - Precargar la URL al pasar el ratón o al cargar la lista. Ya existe precarga para la siguiente de la cola; falta para la canción que se va a pulsar.
   - O mantener un yt-dlp vivo que reciba IDs, para ahorrar los ~400 ms de arranque y la carga de extractores.
   - O resolver la URL en Rust con rustypipe cuando no hay desafío, usando yt-dlp solo como respaldo.
3. **Hallazgo aparte (2 de 10):** `engine.load` tardó 237 ms y 1 242 ms después de tener 384 KB. Lo más probable es que el decodificador necesite más bytes de los que hay para sondear el MP4 y se quede esperando en el lector. Esa espera es síncrona en el hilo de trabajo, así que mientras dura no se atienden otros comandos (pausa, siguiente…). Falta confirmarlo.
4. **t2→t3 varía de 18 a 602 ms:** es la primera conexión TLS a un servidor de googlevideo distinto en cada canción.

## Pendiente (otros pasos del plan)

- Clic en Inicio → contenido (~1 s): no se instrumentó en este paso.
- Primer arranque tras actualizar (2,5 s): sin verificar si es Defender.
