// 每次构建自增 build.count，并把「版本号+编译次数」（如 1.1.15+43）写回各配置文件，
// 同时生成 src/version.gen.ts 供前端展示构建号。
//
// 由 tauri.conf.json 的 beforeBuildCommand 在每次 `tauri build` 前自动执行；
// `tauri dev` 不经过这里，开发态保持基础版本号。
// +N 是 semver 构建元数据：自动更新器比较版本时只看 1.1.15 部分，完全兼容。
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const countFile = resolve(root, 'build.count');
const previous = existsSync(countFile)
  ? parseInt(readFileSync(countFile, 'utf8').trim(), 10) || 0
  : 0;
const count = previous + 1;
writeFileSync(countFile, `${count}\n`);

// 基础版本从 tauri.conf.json 读（剥掉历史 +N），保证所有文件写出的串一致。
const tauriConfPath = resolve(root, 'src-tauri/tauri.conf.json');
const tauriConf = JSON.parse(readFileSync(tauriConfPath, 'utf8'));
const base = String(tauriConf.version).split('+')[0];
const version = `${base}+${count}`;

const replaceFirst = (text, from, to) => {
  const idx = text.indexOf(from);
  if (idx < 0) throw new Error(`stamp-build: 找不到待替换串「${from}」`);
  return text.slice(0, idx) + to + text.slice(idx + from.length);
};

const jsonJobs = [
  { file: tauriConfPath, from: `"version": "${tauriConf.version}"` },
  {
    file: resolve(root, 'package.json'),
    from: `"version": "${tauriConf.version}"`,
  },
  {
    file: resolve(root, 'src-tauri/tauri.macos.conf.json'),
    from: `"version": "${tauriConf.version}"`,
  },
];
for (const { file, from } of jsonJobs) {
  const text = readFileSync(file, 'utf8');
  writeFileSync(file, replaceFirst(text, from, `"version": "${version}"`));
}

// Cargo.toml 只替换 [package] 的 version（文件里第一个 `version = "..."`）。
const cargoTomlPath = resolve(root, 'src-tauri/Cargo.toml');
const cargoToml = readFileSync(cargoTomlPath, 'utf8');
writeFileSync(
  cargoTomlPath,
  replaceFirst(cargoToml, `version = "${tauriConf.version}"`, `version = "${version}"`),
);

const versionGen = `// 由 scripts/stamp-build.mjs 生成，勿手改；提交入库保证 tsc 在全新克隆上可编译。
export const BUILD_NUMBER = ${count};
`;
writeFileSync(resolve(root, 'src/version.gen.ts'), versionGen);

console.log(`[stamp-build] version=${version} (build.count: ${previous} -> ${count})`);
