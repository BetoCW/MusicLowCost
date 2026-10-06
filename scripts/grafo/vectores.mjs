// Capa vectorial del motor híbrido de YoutubeInRustWeb. Heredada de KodBrowser (y este de
// BlackImperial): cubre la documentación de diseño, que es donde se explica el porqué de
// cada decisión (cabeceras `//!` de módulo, README, notas).
//
// Qué se vectoriza:
//   1. Los nodos con cuerpo del grafo de CodeGraph —funciones, métodos, structs, enums,
//      traits, alias de tipo y constantes largas—, leyendo su texto por las líneas que el
//      grafo ya resolvió, con su `///` delante.
//   2. La cabecera `//!` de cada módulo .rs. CodeGraph no la guarda como docstring de
//      ningún nodo, y ahí vive la razón de ser del módulo.
//   3. Las secciones de los .md y .txt (README, Instruction, Objetives…), partidas por
//      encabezado. Cada sección lleva la ruta de encabezados que la contiene, porque el
//      README repite subtítulos («Coste», «Límites declarados») bajo cada eje.
//
// Cada fragmento guarda su vector y su texto; el texto alimenta un FTS5 propio, para
// que el ranking léxico y el vectorial compitan sobre el MISMO universo (el FTS5 de
// CodeGraph no contiene la documentación y la dejaba siempre atrás).
//
// Cómo se mantiene barato:
//   · Incremental por archivo: el hash de contenido de CodeGraph (o sha256 de los .md y
//     .txt) decide qué archivos se vuelven a trocear. Lo demás ni se lee.
//   · Incremental por fragmento: dentro de un archivo editado, el fragmento cuyo texto
//     no cambió reutiliza su vector por hash. Solo se embebe lo que cambió de verdad.
//   · Por lotes: se embebe de LOTE en LOTE, ordenado por longitud (menos relleno) y cada
//     grupo de archivos se escribe en una sola transacción. Si el proceso muere a medias,
//     lo no confirmado se rehace después.
//   · Perezoso: el modelo (~120 MB) solo se carga si hay algo que embeber o que buscar.
//
// No importa node:sqlite: quien llama pasa DatabaseSync (ver buscar.mjs).

import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';

export const MODELO = 'Xenova/multilingual-e5-small';
const ESQUEMA = 1;
// Medido en BlackImperial (96 fragmentos, CPU): lote 16 × 1800 caracteres → 5,3 frag/s y
// 1 457 MB de pico; lote 4 × 1200 → 7,8 frag/s y 654 MB. En CPU el relleno al texto más
// largo del lote cuesta más de lo que ahorra agrupar: lotes chicos y ordenados por longitud.
const LOTE = 4;
const GRUPO = 64; // fragmentos por transacción: aquí sí rinde agrupar
const MAX = 1200; // ~400 tokens; cabecera y docstring van primero, que es lo que más pesa
const CORTA = 200; // secciones de documentación más cortas se juntan con la siguiente
const CON_CUERPO = new Set(['function', 'method', 'class', 'struct', 'enum', 'trait', 'interface', 'type_alias']);
const DOCUMENTOS = new Set(['.md', '.txt']);
// vendor/ (crates de terceros con parches) y version-webview/ (versión anterior) no son
// código de la app: tampoco están en el grafo (codegraph.json).
const SALTAR = new Set(['node_modules', '.git', '.codegraph', 'target', '.claude', 'vendor', 'version-webview']);

const sha = (texto) => createHash('sha256').update(texto).digest('hex');
const aBlob = (v) => new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
export const aVector = (blob) => new Float32Array(blob.slice().buffer);

function leer(raiz, ruta) {
  try {
    return readFileSync(path.join(raiz, ruta), 'utf8');
  } catch {
    return null;
  }
}

function transaccion(db, fn) {
  db.exec('begin');
  try {
    fn();
    db.exec('commit');
  } catch (error) {
    db.exec('rollback');
    throw error;
  }
}

export function abrirVectores(DatabaseSync, raiz) {
  const db = new DatabaseSync(path.join(raiz, '.codegraph', 'vectores.db'));
  db.exec(`
    pragma journal_mode = wal;
    create table if not exists meta (clave text primary key, valor text not null);
  `);
  // Otro modelo (vectores no comparables) u otro esquema: se tira todo y se reconstruye.
  const version = `${MODELO}#${ESQUEMA}`;
  if (db.prepare("select valor from meta where clave = 'version'").get()?.valor !== version) {
    db.exec(`
      drop trigger if exists fragmentos_ai;
      drop trigger if exists fragmentos_ad;
      drop table if exists fragmentos_fts;
      drop table if exists fragmentos;
      drop table if exists archivos;
    `);
    db.prepare("insert or replace into meta (clave, valor) values ('version', ?)").run(version);
  }
  db.exec(`
    create table if not exists archivos (ruta text primary key, hash text not null);
    create table if not exists fragmentos (
      id text primary key,
      ruta text not null,
      tipo text not null,
      nombre text not null,
      linea_ini integer not null,
      linea_fin integer not null,
      nodo text,
      hash text not null,
      texto text not null,
      vector blob not null
    );
    create index if not exists fragmentos_ruta on fragmentos (ruta);
    create index if not exists fragmentos_hash on fragmentos (hash);
    create virtual table if not exists fragmentos_fts using fts5 (
      nombre, texto, content = 'fragmentos', content_rowid = 'rowid',
      tokenize = 'unicode61 remove_diacritics 2'
    );
    create trigger if not exists fragmentos_ai after insert on fragmentos begin
      insert into fragmentos_fts (rowid, nombre, texto) values (new.rowid, new.nombre, new.texto);
    end;
    create trigger if not exists fragmentos_ad after delete on fragmentos begin
      insert into fragmentos_fts (fragmentos_fts, rowid, nombre, texto) values ('delete', old.rowid, old.nombre, old.texto);
    end;
  `);
  return db;
}

let extractor = null;
export async function embeber(raiz, textos) {
  if (!extractor) {
    const { pipeline, env } = await import('@huggingface/transformers');
    env.cacheDir = path.join(raiz, '.codegraph', 'modelos');
    extractor = await pipeline('feature-extraction', MODELO, { dtype: 'q8', device: 'cpu' });
  }
  const salida = await extractor(textos, { pooling: 'mean', normalize: true });
  const [n, dim] = salida.dims;
  return Array.from({ length: n }, (_, i) => salida.data.slice(i * dim, (i + 1) * dim));
}

// Un bloque largo (una sección de 80 líneas, una cabecera `//!` de 45) se parte en
// ventanas de MAX caracteres por líneas, y cada ventana lleva el título del bloque.
function ventanas({ ruta, tipo, titulo, lineas, linea_ini, prefijo }) {
  const salida = [];
  for (let desde = 0; desde < lineas.length; ) {
    let hasta = desde;
    let largo = 0;
    while (hasta < lineas.length && (hasta === desde || largo + lineas[hasta].length < MAX)) largo += lineas[hasta++].length + 1;
    const cuerpo = `${titulo}${desde ? ' (continuación)' : ''}\n${lineas.slice(desde, hasta).join('\n')}`;
    salida.push({
      id: `${prefijo}#${salida.length}`,
      ruta,
      tipo,
      nombre: titulo.slice(0, 120),
      linea_ini: linea_ini + desde,
      linea_fin: linea_ini + hasta - 1,
      nodo: null,
      texto: `${tipo} · ${ruta}\n${cuerpo}`.slice(0, MAX),
    });
    desde = hasta;
  }
  return salida;
}

// La cabecera `//!` de un módulo: las líneas seguidas al principio del archivo.
function cabeceraDeModulo(ruta, lineas) {
  let fin = 0;
  while (fin < lineas.length && /^\s*\/\/!/.test(lineas[fin])) fin++;
  if (!fin) return [];
  const texto = lineas.slice(0, fin).map((l) => l.replace(/^\s*\/\/!\s?/, ''));
  if (!texto.some((l) => /\p{L}/u.test(l))) return [];
  const titulo = `módulo ${ruta.replace(/^crates\//, '').replace(/\/src\//, '::').replace(/\.rs$/, '')}: ${texto.find((l) => l.trim()).trim()}`;
  return ventanas({ ruta, tipo: 'modulo', titulo, lineas: texto, linea_ini: 1, prefijo: `mod:${ruta}` });
}

function fragmentosDeGrafo(grafo, raiz, ruta) {
  const lineas = leer(raiz, ruta)?.split(/\r?\n/);
  if (!lineas) return [];
  const nodos = grafo
    .prepare('select id, kind, name, qualified_name, start_line, end_line, docstring from nodes where file_path = ? order by start_line')
    .all(ruta)
    .filter((n) => CON_CUERPO.has(n.kind) || (n.kind === 'constant' && n.end_line - n.start_line >= 4));
  return [
    ...(ruta.endsWith('.rs') ? cabeceraDeModulo(ruta, lineas) : []),
    ...nodos.map((n) => ({
      id: n.id,
      ruta,
      tipo: n.kind,
      nombre: n.name,
      linea_ini: n.start_line,
      linea_fin: n.end_line,
      nodo: n.id,
      texto: [`${n.kind} ${n.qualified_name} · ${ruta}`, n.docstring, lineas.slice(n.start_line - 1, n.end_line).join('\n')]
        .filter(Boolean)
        .join('\n')
        .slice(0, MAX),
    })),
  ];
}

// Secciones de un documento. En .md, por encabezado `#`, llevando la ruta de
// encabezados («Motor de renderizado › Posicionamiento › Coste»). En .txt, por líneas
// en mayúsculas («METAS A CORTO PLAZO»), que es como Objetives.txt titula.
// Los bloques de código ``` no se miran: un `# comentario` dentro no abre sección.
export function seccionesDeDocumento(ruta, texto) {
  const lineas = texto.split(/\r?\n/);
  const esMd = ruta.endsWith('.md');
  const pila = [];
  const secciones = [];
  let actual = { titulo: path.basename(ruta), desde: 0 };
  let enCodigo = false;
  const cerrar = (hasta) => {
    const cuerpo = lineas.slice(actual.desde, hasta);
    if (cuerpo.some((l) => /\p{L}/u.test(l))) secciones.push({ titulo: actual.titulo, lineas: cuerpo, linea_ini: actual.desde + 1 });
  };
  lineas.forEach((l, i) => {
    if (esMd && /^\s*(```|~~~)/.test(l)) enCodigo = !enCodigo;
    if (enCodigo) return;
    let nivel = 0;
    let nombre = '';
    const md = esMd && /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(l);
    if (md) {
      nivel = md[1].length;
      nombre = md[2];
    } else if (!esMd && l.trim().length >= 6 && /\p{Lu}/u.test(l) && l === l.toUpperCase() && !/[.;:]$/.test(l.trim())) {
      nivel = 1;
      nombre = l.trim();
    }
    if (!nivel) return;
    cerrar(i);
    while (pila.length && pila.at(-1).nivel >= nivel) pila.pop();
    pila.push({ nivel, nombre });
    actual = { titulo: pila.map((p) => p.nombre).join(' › '), desde: i };
  });
  cerrar(lineas.length);
  return secciones;
}

function fragmentosDeDocumento(raiz, ruta) {
  const texto = leer(raiz, ruta);
  if (texto == null) return [];
  const fragmentos = [];
  let pendiente = null;
  const emitir = (s) =>
    fragmentos.push(...ventanas({ ruta, tipo: 'doc', titulo: s.titulo, lineas: s.lineas, linea_ini: s.linea_ini, prefijo: `doc:${ruta}#${fragmentos.length}` }));
  for (const s of seccionesDeDocumento(ruta, texto)) {
    pendiente = pendiente ? { ...pendiente, lineas: [...pendiente.lineas, ...s.lineas] } : s;
    if (pendiente.lineas.join('\n').length >= CORTA) {
      emitir(pendiente);
      pendiente = null;
    }
  }
  if (pendiente) emitir(pendiente);
  return fragmentos;
}

function archivosDeDocumento(raiz, dir = '') {
  const salida = [];
  for (const entrada of readdirSync(path.join(raiz, dir), { withFileTypes: true })) {
    if (SALTAR.has(entrada.name)) continue;
    const rel = dir ? `${dir}/${entrada.name}` : entrada.name;
    if (entrada.isDirectory()) salida.push(...archivosDeDocumento(raiz, rel));
    else if (DOCUMENTOS.has(path.extname(entrada.name).toLowerCase())) salida.push(rel);
  }
  return salida;
}

export async function sincronizar({ raiz, grafo, db, solo, avisar = () => {} }) {
  const dentro = (ruta) => !solo || ruta.startsWith(solo);

  const actuales = new Map();
  for (const f of grafo.prepare('select path, content_hash from files').all()) if (dentro(f.path)) actuales.set(f.path, f.content_hash);
  for (const ruta of archivosDeDocumento(raiz)) {
    const texto = dentro(ruta) ? leer(raiz, ruta) : null;
    if (texto != null) actuales.set(ruta, sha(texto));
  }
  const previos = new Map(db.prepare('select ruta, hash from archivos').all().filter((a) => dentro(a.ruta)).map((a) => [a.ruta, a.hash]));

  const borrarFragmentos = db.prepare('delete from fragmentos where ruta = ?');
  const borrarArchivo = db.prepare('delete from archivos where ruta = ?');
  const borrados = [...previos.keys()].filter((ruta) => !actuales.has(ruta));
  if (borrados.length) {
    transaccion(db, () => {
      for (const ruta of borrados) {
        borrarFragmentos.run(ruta);
        borrarArchivo.run(ruta);
      }
    });
  }

  const cambiados = [...actuales].filter(([ruta, hash]) => previos.get(ruta) !== hash);
  const cuenta = { archivos: cambiados.length, borrados: borrados.length, embebidos: 0, reutilizados: 0 };
  if (!cambiados.length) return cuenta;

  const porHash = db.prepare('select vector from fragmentos where hash = ? limit 1');
  const insertar = db.prepare(
    'insert or replace into fragmentos (id, ruta, tipo, nombre, linea_ini, linea_fin, nodo, hash, texto, vector) values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)',
  );
  const guardarArchivo = db.prepare('insert or replace into archivos (ruta, hash) values (?, ?)');

  let grupo = [];
  let enGrupo = 0;
  let hechos = 0;
  const volcar = async () => {
    const faltan = new Map();
    for (const a of grupo) for (const f of a.fragmentos) if (!f.vector) faltan.set(f.hash, f.texto);
    const hashes = [...faltan.keys()].sort((a, b) => faltan.get(a).length - faltan.get(b).length);
    const nuevos = new Map();
    for (let i = 0; i < hashes.length; i += LOTE) {
      const lote = hashes.slice(i, i + LOTE);
      const vectores = await embeber(raiz, lote.map((h) => `passage: ${faltan.get(h)}`));
      lote.forEach((h, j) => nuevos.set(h, aBlob(vectores[j])));
    }
    transaccion(db, () => {
      for (const a of grupo) {
        borrarFragmentos.run(a.ruta);
        for (const f of a.fragmentos) {
          insertar.run(f.id, f.ruta, f.tipo, f.nombre, f.linea_ini, f.linea_fin, f.nodo, f.hash, f.texto, f.vector ?? nuevos.get(f.hash));
        }
        guardarArchivo.run(a.ruta, a.hash);
      }
    });
    cuenta.embebidos += hashes.length;
    hechos += grupo.length;
    if (cambiados.length > grupo.length) avisar(`vectores ${hechos}/${cambiados.length} archivos`);
    grupo = [];
    enGrupo = 0;
  };

  for (const [ruta, hash] of cambiados) {
    const crudos = DOCUMENTOS.has(path.extname(ruta).toLowerCase()) ? fragmentosDeDocumento(raiz, ruta) : fragmentosDeGrafo(grafo, raiz, ruta);
    const fragmentos = crudos.map((f) => {
      const h = sha(f.texto);
      const vector = porHash.get(h)?.vector;
      if (vector) cuenta.reutilizados++;
      return { ...f, hash: h, vector };
    });
    grupo.push({ ruta, hash, fragmentos });
    enGrupo += fragmentos.length;
    if (enGrupo >= GRUPO) await volcar();
  }
  if (grupo.length) await volcar();
  return cuenta;
}
