// 生成 em-master 的占位应用图标（Wave 0 构建所需：tauri-build 在 Windows 上需要一个 .ico）。
// 纯 Node 实现：手写 PNG 编码，不引入任何图像库，离线可重复生成。
// 后续做正式品牌图标时，用 `npm run tauri icon <源图>` 覆盖 src-tauri/icons/ 即可。
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";

const SIZE = 1024;
const OUT = resolve(process.argv[2] ?? "src-tauri/icons/icon-source.png");

/** CRC32（PNG 分块校验用）。 */
const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** 圆角矩形的有向距离场（负值在内部）。 */
function roundedRectSdf(x, y, cx, cy, halfW, halfH, r) {
  const dx = Math.abs(x - cx) - (halfW - r);
  const dy = Math.abs(y - cy) - (halfH - r);
  const outside = Math.hypot(Math.max(dx, 0), Math.max(dy, 0));
  return outside + Math.min(Math.max(dx, dy), 0) - r;
}

/** 点到线段的距离。 */
function segmentDistance(px, py, ax, ay, bx, by) {
  const vx = bx - ax;
  const vy = by - ay;
  const wx = px - ax;
  const wy = py - ay;
  const len2 = vx * vx + vy * vy;
  const s = len2 === 0 ? 0 : Math.min(Math.max((wx * vx + wy * vy) / len2, 0), 1);
  return Math.hypot(wx - vx * s, wy - vy * s);
}

/** 把距离场换算成 0..1 的抗锯齿覆盖率。 */
function coverage(distance, edge) {
  return Math.min(Math.max(0.5 - distance / edge, 0), 1);
}

const px = Buffer.alloc(SIZE * SIZE * 4);
const cx = SIZE / 2;
const cy = SIZE / 2;
const EDGE = 1.6;

for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const fx = x + 0.5;
    const fy = y + 0.5;

    // 1) 底板：深靛蓝圆角方块，自上而下轻微提亮。
    const plate = coverage(roundedRectSdf(fx, fy, cx, cy, 460, 460, 108), EDGE);
    const t = fy / SIZE;
    const plateR = 26 + 18 * t;
    const plateG = 32 + 22 * t;
    const plateB = 68 + 46 * t;

    // 2) 信封主体：白色圆角矩形。
    const envCy = cy + 6;
    const envHalfH = 205;
    const envTop = envCy - envHalfH;
    const env = coverage(roundedRectSdf(fx, fy, cx, envCy, 300, envHalfH, 34), EDGE);

    // 3) 折角：两条从上方两角收拢到中心的斜线，画在信封之上、用底色压出 V 形开口。
    const apexY = envCy - 30;
    const flapDistance = Math.min(
      segmentDistance(fx, fy, cx - 300, envTop, cx, apexY),
      segmentDistance(fx, fy, cx + 300, envTop, cx, apexY),
    );
    const flap = coverage(flapDistance - 17, EDGE) * env;

    // 4) 合成。
    let r = plateR * (1 - env) + 250 * env;
    let g = plateG * (1 - env) + 251 * env;
    let b = plateB * (1 - env) + 252 * env;

    r = r * (1 - flap) + plateR * 0.72 * flap;
    g = g * (1 - flap) + plateG * 0.72 * flap;
    b = b * (1 - flap) + plateB * 0.72 * flap;

    const i = (y * SIZE + x) * 4;
    px[i] = Math.round(r);
    px[i + 1] = Math.round(g);
    px[i + 2] = Math.round(b);
    px[i + 3] = Math.round(plate * 255);
  }
}

// 组装 PNG：每行前置一个 filter 字节（0 = None）。
const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let y = 0; y < SIZE; y++) {
  raw[y * (SIZE * 4 + 1)] = 0;
  px.copy(raw, y * (SIZE * 4 + 1) + 1, y * SIZE * 4, (y + 1) * SIZE * 4);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8;   // 位深
ihdr[9] = 6;   // 颜色类型 RGBA
ihdr[10] = 0;  // 压缩方法
ihdr[11] = 0;  // 过滤方法
ihdr[12] = 0;  // 隔行扫描

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, png);
console.log(`已生成 ${OUT}（${SIZE}x${SIZE}, ${(png.length / 1024).toFixed(1)} KB）`);