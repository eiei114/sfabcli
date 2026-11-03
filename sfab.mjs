#!/usr/bin/env node
// sfab — Sketchfab を CLI で操作する（ログイン済みの Chrome を CDP で借りて、そのページの中から Sketchfab の API を叩く）。
//   前提: ログイン済みの Chrome が CDP で開いている（既定 127.0.0.1:9223、環境変数 SFAB_CDP で変更）
//           fab / spliceclaw と同じ常駐プロファイルを共有する。Cookie は一切表示しない。
//
//   sfab login                                  常駐 Chrome で Sketchfab のログイン画面を開く
//   sfab status                                 ログイン確認（ユーザ名・アカウント種別）
//   sfab search <語> [--downloadable] [--license by|by-sa|by-nc|cc0] [--animated]
//                [--staffpicked] [--max-face N] [--sort relevance|likes|recent] [--limit 24] [--details] [--json]
//   sfab show <uid|URL>                         詳細（ライセンス・DL可否・形式・サイズ・作者）
//   sfab download <uid|URL> [--format glb|gltf|source|usdz] [--tex 1024|2048|4096]
//                [--out DIR] [--unzip] [--only <name>]   実際に取得する
//   sfab plan <uid|URL> [--format ...]          取得予定を出すだけ（ダウンロードしない）
//   sfab archive-types <uid|URL>                そのモデルで取れる形式とサイズ一覧
//
// 設計: fab CLI と同じ「ログイン済みブラウザの fetch を借りる」方式。OAuth トークン不要。
//       取得できるのは glTF / GLB / USDZ / source。FBX/OBJ 単体は公開されない。
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

const CDP = process.env.SFAB_CDP || 'http://127.0.0.1:9223';
const argv = process.argv.slice(2);
const cmd = argv.shift();

const flag = (n, d) => {
  const i = argv.indexOf('--' + n);
  if (i < 0) return d;
  const v = argv[i + 1];
  argv.splice(i, (v && !v.startsWith('--')) ? 2 : 1);
  return (v && !v.startsWith('--')) ? v : true;
};
const uidOf = (s) => {
  const m = String(s).match(/[0-9a-f]{32}/);
  return m ? m[0] : (String(s).match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/) || [])[0] || s;
};
const die = (m) => { console.error('sfab: ' + m); process.exit(1); };
const mb = (b) => (b / 1e6).toFixed(1) + 'MB';

async function page() {
  let tabs;
  try { tabs = await (await fetch(CDP + '/json')).json(); }
  catch { die(`CDP に繋がらない (${CDP})。ログイン済みの Chrome を --remote-debugging-port 付きで開く`); }
  let t = tabs.find((t) => t.type === 'page' && /^https:\/\/sketchfab\.com/.test(t.url));
  if (!t) {
    t = await (await fetch(CDP + '/json/new?https://sketchfab.com/feed', { method: 'PUT' })).json();
    await new Promise((r) => setTimeout(r, 4000));
  }
  const ws = new WebSocket(t.webSocketDebuggerUrl);
  await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  let id = 0;
  const pend = new Map();
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); }
  };
  const send = (method, params) => new Promise((r) => {
    const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method, params }));
  });
  return {
    ws,
    async eval(expr) {
      const m = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
      if (m.result?.exceptionDetails) die('ページ内エラー: ' + JSON.stringify(m.result.exceptionDetails).slice(0, 300));
      return m.result?.result?.value;
    },
    async api(url) {
      const expr = `(async()=>{const r=await fetch(${JSON.stringify(url)},{credentials:'include',headers:{'accept':'application/json'}});
        const t=await r.text();return JSON.stringify({status:r.status,body:t})})()`;
      const raw = await this.eval(expr);
      const { status, body } = JSON.parse(raw);
      let json = null;
      try { json = JSON.parse(body); } catch {}
      return { status, json, text: body };
    },
  };
}

const out = (o, asJson) => console.log(asJson ? JSON.stringify(o, null, 1) : o);

function licInfo(lic) {
  if (!lic) return { slug: null, label: null, requirements: null, free: null };
  return {
    slug: lic.slug ?? null,
    label: lic.label ?? null,
    requirements: lic.requirements ?? null,
    free: lic.free ?? (/^(cc0|by|by-sa|by-nd|by-nc|by-nc-sa|by-nc-nd|pd)$/i.test(lic.slug ?? '')),
  };
}

async function archiveTypes(P, uid) {
  const r = await P.api(`/i/models/${uid}/download`);
  if (r.status !== 200) return { status: r.status, error: r.json?.detail || r.text?.slice(0, 200), types: null };
  const latest = r.json?.latest ?? {};
  const types = {};
  for (const [k, arr] of Object.entries(latest)) {
    types[k] = (arr || []).map((x) => ({ url: x.url, size: x.size, tex: x.textureMaxResolution ?? null }));
  }
  return { status: 200, authorized: r.json?.status, types };
}

// download JSON の URL は /i/... を返すので、実体（S3 署名付き URL）を1段解決する
async function resolveUrl(P, rel) {
  if (!rel) return null;
  if (/^https:/.test(rel)) return rel;
  const r = await P.api(rel);
  if (r.status !== 200) die(`URL 解決に失敗 (HTTP ${r.status})`);
  return r.json?.url ?? null;
}

if (cmd === 'login') {
  await fetch(CDP + '/json/new?https://sketchfab.com/login', { method: 'PUT' }).catch(() => die('CDP に繋がらない'));
  console.log('ログイン画面を開いた。終わったら sfab status');
  process.exit(0);
}

if (!cmd || cmd === 'help' || cmd === '--help' || cmd === '-h') {
  console.log(fs.readFileSync(new URL(import.meta.url)).toString()
    .split('\n').filter((l) => l.startsWith('//')).map((l) => l.slice(3)).join('\n'));
  process.exit(0);
}

const P = await page();
try {
  if (cmd === 'status') {
    const r = await P.api('/i/users/me');
    if (r.status !== 200) { console.log(`未ログイン (HTTP ${r.status})。sfab login`); process.exit(1); }
    console.log(`ok: ${r.json.displayName || r.json.username}（${r.json.account}・ログイン済み）`);
  }

  else if (cmd === 'search') {
    const q = argv.filter((a) => !a.startsWith('--')).join(' ');
    const lim = Math.min(24, +flag('limit', 24));
    const asJson = flag('json', false);
    const sort = flag('sort', 'relevance');
    const lic = flag('license', null);
    const animated = flag('animated', false);
    const staff = flag('staffpicked', false);
    const maxFace = flag('max-face', null);
    const downloadable = flag('downloadable', true);

    const qs = new URLSearchParams({ type: 'models', q, count: String(lim) });
    if (downloadable) qs.set('downloadable', 'true');
    if (sort === 'likes') qs.set('sort_by', '-likeCount');
    else if (sort === 'recent') qs.set('sort_by', '-publishedAt');
    else qs.set('sort_by', '-relevance');
    // /i/search の license パラメータは効かない（実測）。--details と併用したときだけクライアント側で絞る。
    const licFilter = lic ? String(lic).split(',') : null;
    if (animated) qs.set('animated', 'true');
    if (staff) qs.set('staffpicked', 'true');
    if (maxFace) qs.set('max_face_count', String(maxFace));

    const r = await P.api('/i/search?' + qs);
    if (r.status !== 200) die(`search HTTP ${r.status}: ${r.text.slice(0, 200)}`);
    const results = r.json?.results || [];
    // 検索結果に license が入っていないので、--details を付けたときだけ個別取得で補う（N+1 を抑える）
    const wantDetails = flag('details', false);
    const detailOf = async (uid) => {
      const d = await P.api(`/i/models/${uid}`);
      return d.status === 200 ? d.json : null;
    };
    const rows = [];
    for (const m of results) {
      let lic = m.license;
      let faces = m.faceCount ?? null;
      let ai = m.isAiGenerated ?? null;
      if (wantDetails) {
        const d = await detailOf(m.uid);
        if (d) { lic = d.license; faces = d.faceCount ?? faces; ai = d.isAiGenerated ?? ai; }
      }
      rows.push({
        uid: m.uid,
        name: m.name,
        slug: m.slug,
        by: m.user?.displayName || m.user?.username,
        license: licInfo(lic).slug,
        licenseRequirements: licInfo(lic).requirements,
        dl: m.downloadType ?? null,
        dlCount: m.downloadCount ?? null,
        likes: m.likeCount ?? null,
        faces,
        animated: (m.animationCount ?? 0) > 0,
        ai,
        price: m.price ?? null,
        url: m.viewerUrl
          ? (/^https?:/.test(m.viewerUrl) ? m.viewerUrl : 'https://sketchfab.com' + m.viewerUrl)
          : `https://sketchfab.com/3d-models/${m.uid}`,
      });
    }
    let shown = rows;
    if (licFilter) {
      if (!wantDetails) die('--license は --details と併用する（検索結果にライセンスが含まれないため）');
      shown = rows.filter((m) => licFilter.includes(String(m.license)));
    }
    const rowsOut = shown;
    if (asJson) out(rowsOut, true);
    else for (const m of rowsOut) {
      const tag = [m.license || 'licence?', m.dl || 'dl?', m.faces != null ? `${m.faces}f` : '', m.animated ? 'anim' : ''].filter(Boolean).join(' · ');
      console.log(`${m.uid}  ${tag}  ${m.name}  — ${m.by}`);
    }
    if (!asJson) console.log(`\n${rows.length} 件。詳細は sfab show <uid>`);
  }

  else if (cmd === 'show') {
    const uid = uidOf(argv[0]);
    if (!uid) die('uid か URL が要る');
    const r = await P.api(`/i/models/${uid}`);
    if (r.status !== 200) die(`HTTP ${r.status}`);
    const m = r.json;
    const det = {
      uid: m.uid,
      name: m.name,
      by: m.user?.displayName || m.user?.username,
      license: licInfo(m.license),
      downloadType: m.downloadType ?? null,
      isDownloadable: m.downloadType && m.downloadType !== 'notDownloadable',
      faces: m.faceCount ?? null,
      vertices: m.vertexCount ?? null,
      animations: m.animationCount ?? null,
      textureCount: m.textureCount ?? null,
      aiGenerated: m.isAiGenerated ?? null,
      categories: (m.categories || []).map((c) => c.name || c.slug),
      tags: (m.tags || []).slice(0, 10).map((t) => t.name || t),
      price: m.price ?? null,
      url: `https://sketchfab.com/3d-models/${m.uid}`,
      thumb: m.thumbnails?.images?.[0]?.url ?? null,
    };
    out(det, true);
  }

  else if (cmd === 'archive-types') {
    const uid = uidOf(argv[0]);
    const a = await archiveTypes(P, uid);
    if (a.status !== 200) die(`download 不可 (HTTP ${a.status})${a.error ? ': ' + a.error : ''}`);
    const rows = Object.entries(a.types).map(([k, arr]) => ({
      format: k,
      variants: arr.map((x) => ({ tex: x.tex, size: x.size })),
    }));
    out({ uid, authorized: a.authorized, formats: rows }, true);
  }

  else if (cmd === 'plan' || cmd === 'download') {
    const uid = uidOf(argv[0]);
    if (!uid) die('uid か URL が要る');
    const fmt = String(flag('format', 'glb'));
    const tex = flag('tex', null);
    const dir = path.resolve(flag('out', '.'));
    const unzip = flag('unzip', false);
    const only = flag('only', null);

    const detail = await P.api(`/i/models/${uid}`);
    if (detail.status !== 200) die(`モデルが取れない (HTTP ${detail.status})`);
    const m = detail.json;
    const lic = licInfo(m.license);

    const a = await archiveTypes(P, uid);
    if (a.status !== 200) {
      die(`ダウンロードできない (HTTP ${a.status})。${m.downloadType === 'notDownloadable' ? '作者が DL を禁止している' : a.error || ''}`);
    }
    const variants = a.types[fmt];
    if (!variants || !variants.length) {
      const avail = Object.keys(a.types).filter((k) => a.types[k].length).join(', ');
      die(`形式 ${fmt} は無い。取れる形式: ${avail}`);
    }
    let pick = variants[0];
    if (tex && variants.some((v) => String(v.tex) === String(tex))) {
      pick = variants.find((v) => String(v.tex) === String(tex));
    } else if (variants.length > 1 && variants.some((v) => v.tex)) {
      // 既定は最小テクスチャ。Game 用の取り込みで 4K は不要
      pick = variants.reduce((lo, v) => ((v.size ?? Infinity) < (lo.size ?? Infinity) ? v : lo), variants[0]);
    }

    const plan = {
      uid,
      name: m.name,
      by: m.user?.displayName || m.user?.username,
      format: fmt,
      textureMaxResolution: pick.tex,
      sizeBytes: pick.size,
      sizeMB: pick.size ? mb(pick.size) : null,
      license: lic,
      attributionRequired: lic.slug === 'by' || lic.slug === 'by-sa' || lic.slug === 'by-nc' || lic.slug === 'by-nc-sa',
      aiGenerated: m.isAiGenerated ?? null,
      out: dir,
      url: `https://sketchfab.com/3d-models/${uid}`,
    };

    if (cmd === 'plan') { out(plan, true); process.exit(0); }

    const real = await resolveUrl(P, pick.url);
    if (!real) die('署名付き URL が取れなかった');

    fs.mkdirSync(dir, { recursive: true });
    const ext = fmt === 'gltf' || fmt === 'source' ? 'zip' : fmt === 'usdz' ? 'usdz' : 'glb';
    const safe = String(m.name || uid).replace(/[^\w.\- ]+/g, '_').slice(0, 80).trim() || uid;
    const dest = path.join(dir, `${safe}.${ext}`);

    const curl = process.platform === 'win32' && fs.existsSync('C:/Program Files/Git/mingw64/bin/curl.exe')
      ? 'C:/Program Files/Git/mingw64/bin/curl.exe' : 'curl';
    execFileSync(curl, ['-fSL', '--retry', '2', '-o', dest, real], { stdio: ['ignore', 'ignore', 'inherit'] });
    const got = fs.statSync(dest).size;
    if (pick.size && got !== pick.size) die(`サイズ不一致: ${dest}（期待 ${pick.size} / 実際 ${got}）`);
    console.log(`saved ${dest} (${mb(got)})`);

    if (unzip && /\.zip$/i.test(dest)) {
      const target = path.join(dir, path.basename(dest, '.zip'));
      execFileSync('unzip', ['-qo', dest, '-d', target]);
      console.log('unzipped → ' + target);
    }

    // 取得の記録。ライセンスと出典は必ず残す
    const rec = {
      uid,
      title: m.name,
      by: m.user?.displayName || m.user?.username,
      authorUsername: m.user?.username ?? null,
      authorUrl: m.user?.profileUrl
        ? (/^https?:/.test(m.user.profileUrl) ? m.user.profileUrl : 'https://sketchfab.com' + m.user.profileUrl)
        : (m.user?.username ? 'https://sketchfab.com/' + m.user.username : null),
      license: lic.slug,
      licenseLabel: lic.label,
      licenseRequirements: lic.requirements,
      attributionRequired: plan.attributionRequired,
      aiGenerated: plan.aiGenerated,
      format: fmt,
      textureMaxResolution: pick.tex,
      sizeBytes: got,
      file: path.basename(dest),
      source: `https://sketchfab.com/3d-models/${uid}`,
      fetched: new Date().toISOString(),
    };
    const recPath = path.join(dir, `SFAB_SOURCE_${uid.slice(0, 8)}.json`);
    fs.writeFileSync(recPath, JSON.stringify(rec, null, 1));
    console.log(`record ${recPath}`);
    if (plan.attributionRequired) console.log(`ATTRIBUTION: 「${m.name}」by ${rec.by} (${rec.source}) — ${lic.label}`);
  }

  else {
    die(`未知のコマンド: ${cmd}。sfab help`);
  }
} finally {
  P.ws.close();
}
