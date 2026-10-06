// Hook PostToolUse de Claude Code: cuando se edita un archivo de YoutubeInRustWeb, el grafo
// estructural se pone al día con `codegraph sync` (incremental: solo reprocesa lo que
// cambió de hash). Los vectores NO se tocan aquí: se ponen al día perezosamente en la
// siguiente búsqueda semántica, para que editar no cargue el modelo en memoria.
//
// Vive registrado en D:\Portafolio\.claude\settings.local.json porque Claude se abre
// desde D:\Portafolio; por eso filtra la ruta y no hace nada fuera de este proyecto.
// Nunca bloquea la edición: cualquier fallo sale con 0.

import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const RAIZ = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
// Lo que CodeGraph indexa en este repo. Los .md y .txt los cubre la capa vectorial.
const EXTENSIONES = new Set(['.rs', '.js', '.mjs']);

let entrada = '';
for await (const trozo of process.stdin) entrada += trozo;

let archivo;
try {
  archivo = JSON.parse(entrada)?.tool_input?.file_path;
} catch {
  process.exit(0);
}
if (typeof archivo !== 'string') process.exit(0);

const relativa = path.relative(RAIZ, path.resolve(archivo));
const fuera = relativa.startsWith('..') || path.isAbsolute(relativa);
const ruido = /^(node_modules|\.codegraph|\.git|target|vendor|version-webview)[\\/]/.test(relativa);
if (fuera || ruido || !EXTENSIONES.has(path.extname(relativa).toLowerCase())) process.exit(0);

const shim = path.join(RAIZ, 'node_modules', '@colbymchenry', 'codegraph', 'npm-shim.js');
if (!existsSync(shim) || !existsSync(path.join(RAIZ, '.codegraph', 'codegraph.db'))) process.exit(0);

spawnSync(process.execPath, [shim, 'sync', '--quiet', RAIZ], {
  cwd: RAIZ,
  stdio: 'ignore',
  timeout: 60_000,
  env: { ...process.env, CODEGRAPH_TELEMETRY: '0', CODEGRAPH_NO_DAEMON: '1' },
});
process.exit(0);
