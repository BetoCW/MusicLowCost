// Motor híbrido de YoutubeInRustWeb: grafo estructural + vectores en UN solo proceso.
// Heredado de KodBrowser; la capa vectorial cubre la documentación de diseño
// (`//!` de módulo, README).
//
//   node scripts/grafo/buscar.mjs "¿dónde se colapsan las resoluciones DNS?"
//   node scripts/grafo/buscar.mjs "dónde se resuelve el audio con yt-dlp" --k 5 --solo src/jam/ --json
//   node scripts/grafo/buscar.mjs --indexar [--solo src/jam/]
//
// Es un proceso corto: pone al día el grafo (codegraph sync, incremental), pone al día
// los vectores (solo lo que cambió), responde y suelta la memoria. No hay servidor
// residente que se caiga ni que ocupe RAM entre consultas.
//
// Ranking: fusión por rango recíproco (RRF) de dos listas sobre los mismos fragmentos
// —coseno sobre los vectores y BM25 sobre su texto—. Lo que `codegraph.json` marca como
// `deprioritize` (los ejemplos) cuenta la mitad. Cada resultado que es un nodo del grafo
// se expande con quién lo llama y a quién llama.
//
// Las preguntas estructurales exactas no necesitan este motor:
//   npx codegraph callers <símbolo>   ·   npx codegraph impact <símbolo>
//
// Requiere Node >= 22.5 (node:sqlite).

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { abrirVectores, aVector, embeber, sincronizar } from './vectores.mjs';

const RAIZ = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const GRAFO = path.join(RAIZ, '.codegraph', 'codegraph.db');
const RRF = 60;
const CANDIDATOS = 50;
const PARADA = new Set([
  'donde', 'dónde', 'como', 'cómo', 'que', 'qué', 'cual', 'cuál', 'quien', 'quién', 'cuando', 'cuándo',
  'para', 'por', 'con', 'sin', 'los', 'las', 'del', 'una', 'uno', 'unos', 'unas', 'este', 'esta', 'ese', 'esa',
  'hace', 'hacen', 'puede', 'the', 'and', 'where', 'how', 'what', 'which', 'does', 'with', 'from', 'this', 'that',
]);

const { values: op, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    k: { type: 'string', default: '8' },
    solo: { type: 'string' },
    json: { type: 'boolean', default: false },
    indexar: { type: 'boolean', default: false },
    'sin-sync': { type: 'boolean', default: false },
  },
});
const consulta = positionals.join(' ').trim();
if (!consulta && !op.indexar) {
  console.error('uso: node scripts/grafo/buscar.mjs "consulta" [--k 8] [--solo ruta/] [--json] [--sin-sync]\n     node scripts/grafo/buscar.mjs --indexar [--solo ruta/]');
  process.exit(2);
}
const normalizar = (ruta) => ruta.replace(/\\/g, '/').replace(/^\.\//, '');
const solo = op.solo && normalizar(op.solo);
const k = Math.max(1, Number.parseInt(op.k, 10) || 8);
const dentro = (ruta) => !solo || ruta.startsWith(solo);
const log = (mensaje) => process.stderr.write(`grafo · ${mensaje}\n`);

let segundoPlano = [];
try {
  segundoPlano = (JSON.parse(readFileSync(path.join(RAIZ, 'codegraph.json'), 'utf8')).deprioritize ?? []).map(normalizar);
} catch {
  // Sin codegraph.json no hay nada que degradar.
}

// node:sqlite en Node 22 avisa en cada arranque de que es experimental; aquí es ruido.
const emitirAviso = process.emitWarning;
process.emitWarning = (aviso, ...resto) => (String(aviso).includes('SQLite') ? undefined : emitirAviso.call(process, aviso, ...resto));
const { DatabaseSync } = await import('node:sqlite');

// 1 · Grafo al día. Es el único paso fuera de este proceso: CodeGraph es quien escribe su base.
let reloj = Date.now();
if (!op['sin-sync']) {
  const shim = path.join(RAIZ, 'node_modules', '@colbymchenry', 'codegraph', 'npm-shim.js');
  const orden = existsSync(GRAFO) ? ['sync', '--quiet', RAIZ] : ['init', RAIZ];
  const r = spawnSync(process.execPath, [shim, ...orden], {
    cwd: RAIZ,
    stdio: ['ignore', 'ignore', 'inherit'],
    timeout: 600_000,
    env: { ...process.env, CODEGRAPH_TELEMETRY: '0', CODEGRAPH_NO_DAEMON: '1' },
  });
  if (r.status !== 0) log(`codegraph ${orden[0]} falló (${r.status ?? r.signal}); se sigue con el grafo que haya`);
}
if (!existsSync(GRAFO)) {
  log('no hay grafo todavía: npx codegraph init');
  process.exit(1);
}
const grafo = new DatabaseSync(GRAFO, { readOnly: true });
const db = abrirVectores(DatabaseSync, RAIZ);
const msGrafo = Date.now() - reloj;

// 2 · Vectores al día.
reloj = Date.now();
const c = await sincronizar({ raiz: RAIZ, grafo, db, solo, avisar: log });
log(`grafo ${msGrafo} ms · vectores ${Date.now() - reloj} ms (${c.archivos} archivos al día, ${c.embebidos} embebidos, ${c.reutilizados} reutilizados, ${c.borrados} borrados)`);

if (op.indexar) {
  log(`${db.prepare('select count(*) as n from fragmentos').get().n} fragmentos en el índice`);
  cerrar();
  process.exit(0);
}

// 3 · Dos listas de candidatos sobre los mismos fragmentos.
reloj = Date.now();
const [vq] = await embeber(RAIZ, [`query: ${consulta}`]);

const porVector = [];
for (const f of db.prepare('select id, ruta, tipo, nombre, linea_ini, linea_fin, nodo, vector from fragmentos').iterate()) {
  if (!dentro(f.ruta)) continue;
  const v = aVector(f.vector);
  let sim = 0;
  for (let i = 0; i < v.length; i++) sim += v[i] * vq[i];
  porVector.push({ id: f.id, ruta: f.ruta, tipo: f.tipo, nombre: f.nombre, linea_ini: f.linea_ini, linea_fin: f.linea_fin, nodo: f.nodo, sim });
}
porVector.sort((a, b) => b.sim - a.sim);

const terminos = [...new Set(consulta.toLowerCase().match(/[\p{L}\p{N}_]{3,}/gu) ?? [])]
  .filter((w) => !PARADA.has(w))
  .map((w) => (w.length > 4 && w.endsWith('s') ? w.slice(0, -1) : w));
let porTexto = [];
if (terminos.length) {
  porTexto = db
    .prepare(
      `select f.id, f.ruta, f.tipo, f.nombre, f.linea_ini, f.linea_fin, f.nodo
         from fragmentos_fts join fragmentos f on f.rowid = fragmentos_fts.rowid
        where fragmentos_fts match ?
        order by bm25(fragmentos_fts, 4.0, 1.0) limit 200`,
    )
    .all(terminos.map((w) => `"${w.replaceAll('"', '')}"*`).join(' OR '))
    .filter((f) => dentro(f.ruta))
    .slice(0, CANDIDATOS);
}

// 4 · Fusión por rango recíproco.
const fusion = new Map();
const sumar = (lista, rango) =>
  lista.forEach((candidato, i) => {
    const entrada = fusion.get(candidato.id) ?? { ...candidato, puntos: 0 };
    entrada.puntos += 1 / (RRF + i + 1);
    entrada[rango] = i + 1;
    fusion.set(candidato.id, entrada);
  });
sumar(porVector.slice(0, CANDIDATOS), 'rangoVector');
sumar(porTexto, 'rangoLexico');
for (const e of fusion.values()) {
  if (segundoPlano.some((p) => e.ruta.startsWith(p))) {
    e.puntos /= 2;
    e.prioridadBaja = true;
  }
}
const mejores = [...fusion.values()].sort((a, b) => b.puntos - a.puntos).slice(0, k);

// 5 · Expansión estructural de cada resultado que es un nodo del grafo.
const llamadores = grafo.prepare(
  `select distinct s.name, s.file_path as ruta, s.start_line as linea from edges e join nodes s on s.id = e.source
    where e.target = ? and e.kind in ('calls', 'references', 'instantiates') and s.kind not in ('file', 'import') limit 6`,
);
const llamados = grafo.prepare(
  `select distinct t.name, t.file_path as ruta, t.start_line as linea from edges e join nodes t on t.id = e.target
    where e.source = ? and e.kind in ('calls', 'instantiates') and t.kind not in ('file', 'import') limit 6`,
);
const documentacion = grafo.prepare('select docstring from nodes where id = ?');
for (const m of mejores) {
  if (!m.nodo) continue;
  m.llamadoPor = llamadores.all(m.nodo);
  m.llama = llamados.all(m.nodo);
  m.resumen = documentacion
    .get(m.nodo)
    ?.docstring?.split('\n')
    .map((l) => l.replace(/^[^\p{L}\p{N}]+/u, '').trim())
    .find((l) => /\p{L}/u.test(l));
}

if (op.json) {
  console.log(JSON.stringify(mejores, null, 2));
} else {
  const lista = (xs) => xs.map((x) => `${x.name} (${x.ruta}:${x.linea})`).join(', ');
  mejores.forEach((m, i) => {
    const origen = [m.rangoVector && `vector #${m.rangoVector}`, m.rangoLexico && `léxico #${m.rangoLexico}`, m.prioridadBaja && 'prioridad baja']
      .filter(Boolean)
      .join(' · ');
    console.log(`${i + 1}. ${m.ruta}:${m.linea_ini}-${m.linea_fin}  ${m.tipo} ${m.nombre}  [${origen}]`);
    if (m.resumen) console.log(`   ${m.resumen}`);
    if (m.llamadoPor?.length) console.log(`   ← llamado por: ${lista(m.llamadoPor)}`);
    if (m.llama?.length) console.log(`   → llama a: ${lista(m.llama)}`);
  });
}
log(`consulta ${Date.now() - reloj} ms`);
cerrar();

function cerrar() {
  grafo.close();
  db.close();
}
