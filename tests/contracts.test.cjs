// Independent checks against the actual SQLite DDL, Rust SQL, and adapter fixtures.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { DatabaseSync } = require('node:sqlite');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const read = name => fs.readFileSync(path.join(root, name), 'utf8');
const database = () => {
  const db = new DatabaseSync(':memory:');
  db.exec(read('src/storage/schema.sql'));
  return db;
};
function rustFiles(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const item = path.join(dir, entry.name);
    return entry.isDirectory() ? rustFiles(item) : item.endsWith('.rs') ? [item] : [];
  });
}
test('schema opens on SQLite and supports JSON extraction', () => {
  const db = database();
  assert.equal(db.prepare('PRAGMA user_version').get().user_version, 1);
  assert.equal(db.prepare("SELECT json_extract('{\"a\":\"🦀\"}','$.a') AS s").get().s, '🦀');
  db.close();
});
test('SQL statements embedded in Rust prepare against the shipping schema', () => {
  const db = database();
  let count = 0;
  const statement = /"((?:SELECT|INSERT|DELETE|UPDATE)\s+[^"\n]*)"/g;
  for (const file of rustFiles(path.join(root, 'src'))) {
    const source = fs.readFileSync(file, 'utf8');
    for (const [, sql] of source.matchAll(statement)) {
      assert.doesNotThrow(() => db.prepare(sql), path.basename(file) + ': ' + sql);
      count++;
    }
  }
  assert.ok(count >= 25, String(count));
  db.close();
});
test('schema enforces one writer reservation and unique daily numbers', () => {
  const db = database();
  const add = db.prepare('INSERT INTO chunks(day,version,state) VALUES(?,?,?)');
  add.run('2026-11-24', 0, 'active');
  assert.throws(() => add.run('2026-11-25', 0, 'active'));
  assert.throws(() => add.run('2026-11-24', 0, 'sealed'));
  db.exec("UPDATE chunks SET state='sealed'");
  add.run('2026-11-25', 0, 'active');
  assert.equal(db.prepare('SELECT COUNT(*) n FROM chunks').get().n, 2);
  db.close();
});
test('duplicate IDs, invalid states, and orphaned events are rejected', () => {
  const db = database();
  db.exec("INSERT INTO chunks(id,day,version) VALUES(1,'2026-11-24',0)");
  const add = db.prepare('INSERT INTO events(eid,cid,chunk,payload) VALUES(?,?,?,?)');
  add.run('e1','c1',1,'{}');
  assert.throws(() => add.run('e1','c1',1,'{}'));
  assert.throws(() => add.run('e2','c2',999,'{}'));
  assert.throws(() => db.exec("UPDATE chunks SET state='invalid'"));
  db.close();
});
test('membership in one file finds a command and its events in other files', () => {
  const db = database();
  db.exec("INSERT INTO chunks(id,day,version,state) VALUES(1,'2026-11-24',0,'sealed'),(2,'2026-11-24',1,'active')");
  const add = db.prepare('INSERT INTO events(eid,cid,chunk,payload) VALUES(?,?,?,?)');
  for (const row of [['a','one',1],['b','one',2],['c','two',2]]) add.run(...row,'{}');
  const query = read('src/query.rs').match(/"(SELECT cid,MIN\(rowid\) AS first_id[^"]+)"/)[1];
  // SQLite's ?1/?2 bind by name in Node's binding interface.
  const found = db.prepare(query).all({1:1,2:100});
  assert.deepEqual(found.map(x=>x.cid), ['one']);
  assert.equal(db.prepare("SELECT COUNT(*) n FROM events WHERE cid='one'").get().n, 2);
  db.close();
});
test('legacy and native fixture payloads carry distinct command and output semantics', () => {
  const events = JSON.parse(read('tests/fixtures/legacy.json'));
  assert.ok(events.every(e=>e.schemaVersion===1 && e.commandId===events[0].commandId));
  assert.equal(events[1].text, 'hello\n');
  const claude = JSON.parse(read('tests/fixtures/claude.json'));
  assert.equal(claude.tool_response.stderr, 'warning\n');
  const codex = read('tests/fixtures/codex.jsonl').trim().split(/\r?\n/).map(s=>JSON.parse(s));
  assert.equal(codex.find(e=>e.item?.exit_code===7).item.aggregated_output, 'error\n');
});
test('schema-v2 sample and byte accounting round-trip non-ASCII output', () => {
  const samples=JSON.parse(read('tests/fixtures/chunk-v2.json'));
  const serialized=samples.map(e=>JSON.stringify(e));
  const expected=2+serialized.reduce((n,s)=>n+Buffer.byteLength(s,'utf8'),0)+samples.length-1;
  assert.equal(expected,Buffer.byteLength(JSON.stringify(samples),'utf8'));
  assert.ok(samples.every(e=>e.schemaVersion===2 && e.agentApp && e.commandId));
  assert.equal(samples[1].text,'hello 🦀\n');
});

test('independent byte oracle parses an exact 50 MB UTF-8 JSON chunk on disk', () => {
  const os = require('node:os');
  const limit = 50_000_000;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'aw-byte-oracle-'));
  const file = path.join(dir, '2026_11_24_0.json');
  const base = JSON.parse(read('tests/fixtures/chunk-v2.json'))[1];
  const parts = [];
  let bytes = 2;
  const part = '🦀"\n'.repeat(12_000);
  try {
    for (let i = 0; ; i++) {
      const prefix = { ...base, eventId: 'p' + i, sequence: i, text: '' };
      const separator = parts.length ? 1 : 0;
      const full = JSON.stringify({ ...prefix, text: part });
      if (bytes + separator + Buffer.byteLength(full) + 256 >= limit) {
        const fill = limit - bytes - separator - Buffer.byteLength(JSON.stringify(prefix));
        assert.ok(fill >= 0);
        parts.push(JSON.stringify({ ...prefix, text: 'z'.repeat(fill) }));
        break;
      }
      parts.push(full);
      bytes += separator + Buffer.byteLength(full);
    }
    const fd = fs.openSync(file, 'wx');
    try {
      fs.writeSync(fd, '[');
      for (let i = 0; i < parts.length; i++) fs.writeSync(fd, (i ? ',' : '') + parts[i]);
      fs.writeSync(fd, ']');
    } finally { fs.closeSync(fd); }
    assert.equal(fs.statSync(file).size, limit);
    const values = JSON.parse(fs.readFileSync(file, 'utf8'));
    assert.equal(values.length, parts.length);
    assert.equal(values[0].text, part);
    values.at(-1).text += 'z';
    assert.equal(Buffer.byteLength(JSON.stringify(values)), limit + 1);
  } finally {
    if (fs.existsSync(file)) fs.unlinkSync(file);
    fs.rmdirSync(dir);
  }
});
