#!/usr/bin/env python3
"""生成 macOS 菜单栏托盘模板图标 icons/tray-macos.png。

产物规格：36x36 RGBA，颜色通道恒为 0（纯黑），形状**仅由 alpha 通道表达**。
这是 macOS 模板图标（template image）的硬性规范——系统据此在深色/浅色菜单栏
自动反色。直接用彩色应用图标会在深色菜单栏上与背景融为一体，那正是本次修复的原症状。

尺寸依据：tray-icon 把菜单栏图标高度硬编码缩放到 18pt
（tray-icon/src/platform_impl/macos/mod.rs 的 `let icon_height: f64 = 18.0`，
宽度按原始宽高比换算）。故源图取 18pt @2x = 36px，且**必须是正方形**，
否则宽度会按宽高比失真。

图形沿用应用图标的剪贴板意象，保持品牌识别的连续性。

只用标准库（zlib / struct），不引入 Pillow 等第三方依赖——
本脚本与产物一并提交，任何人 clone 后都能无环境准备地复现。
"""

import struct
import sys
import zlib
from pathlib import Path

SIZE = 36          # 最终边长（px）
SS = 8             # 超采样倍率，用于抗锯齿
N = SIZE * SS      # 超采样画布边长


def in_round_rect(px, py, x0, y0, x1, y1, r):
    """点是否落在圆角矩形内。坐标为超采样后的浮点像素中心。"""
    if r <= 0:
        return x0 <= px <= x1 and y0 <= py <= y1
    # 把点向「圆心矩形」夹紧，再比较到夹紧点的距离
    cx = min(max(px, x0 + r), x1 - r)
    cy = min(max(py, y0 + r), y1 - r)
    dx, dy = px - cx, py - cy
    return dx * dx + dy * dy <= r * r


def build_mask():
    """在超采样画布上绘制剪贴板图形，返回 [0,1] 的覆盖率矩阵（SIZE x SIZE）。"""
    # 以 36px 逻辑坐标定义图形，再统一乘以 SS
    def S(v):
        return v * SS

    # 板子外框：描边圆角矩形（外轮廓减内轮廓）
    board_out = (S(7.0), S(7.5), S(29.0), S(33.0), S(3.0))
    stroke = S(2.2)
    board_in = (
        board_out[0] + stroke, board_out[1] + stroke,
        board_out[2] - stroke, board_out[3] - stroke,
        max(board_out[4] - stroke, 0.5),
    )

    # 顶部夹子：实心圆角矩形，压在板子上边缘
    clip = (S(13.0), S(3.2), S(23.0), S(10.5), S(2.0))
    # 夹子周围的透明隔离带，让它在视觉上"浮"在板子之上
    gap = S(1.3)
    clip_gap = (
        clip[0] - gap, clip[1] - gap,
        clip[2] + gap, clip[3] + gap,
        clip[4] + gap,
    )

    # 板内横线：两条，示意文本
    line_x0, line_x1 = S(12.0), S(24.0)
    line_h = S(1.8)
    lines = [
        (line_x0, S(19.0), line_x1, S(19.0) + line_h, line_h / 2),
        (line_x0, S(25.0), S(21.0), S(25.0) + line_h, line_h / 2),
    ]

    cov = [[0.0] * SIZE for _ in range(SIZE)]
    for gy in range(N):
        py = gy + 0.5
        for gx in range(N):
            px = gx + 0.5

            on = False
            # 板子描边
            if in_round_rect(px, py, *board_out) and not in_round_rect(px, py, *board_in):
                on = True
            # 横线
            if not on:
                for ln in lines:
                    if in_round_rect(px, py, *ln):
                        on = True
                        break
            # 夹子隔离带：先挖空
            if on and in_round_rect(px, py, *clip_gap):
                on = False
            # 再画夹子本体
            if not on and in_round_rect(px, py, *clip):
                on = True

            if on:
                cov[gy // SS][gx // SS] += 1.0

    inv = 1.0 / (SS * SS)
    return [[c * inv for c in row] for row in cov]


def write_png(path, mask):
    """把覆盖率矩阵写成 RGBA PNG：颜色通道全 0，alpha 取覆盖率。"""
    path.write_bytes(render_png_bytes(mask))


def render_png_bytes(mask):
    """把覆盖率矩阵渲染成完整的 PNG 字节串。"""
    raw = bytearray()
    for row in mask:
        raw.append(0)  # 每条扫描线的 filter type：None
        for c in row:
            a = int(round(max(0.0, min(1.0, c)) * 255))
            raw += b"\x00\x00\x00" + bytes((a,))

    def chunk(tag, data):
        out = struct.pack(">I", len(data)) + tag + data
        return out + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    # bit_depth=8, color_type=6 (RGBA)
    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def decode_rgba(png_bytes):
    """解出 (width, height, 像素列表)，像素为 (r,g,b,a) 四元组。只处理本脚本自己产出的形态。"""
    pos, idat, width, height = 8, b"", 0, 0
    while pos < len(png_bytes):
        ln = struct.unpack(">I", png_bytes[pos:pos + 4])[0]
        tag = png_bytes[pos + 4:pos + 8]
        data = png_bytes[pos + 8:pos + 8 + ln]
        if tag == b"IHDR":
            width, height = struct.unpack(">II", data[:8])
        elif tag == b"IDAT":
            idat += data
        pos += 12 + ln

    raw = zlib.decompress(idat)
    stride = width * 4 + 1
    pixels = []
    for y in range(height):
        line = raw[y * stride + 1:(y + 1) * stride]
        for x in range(width):
            pixels.append(tuple(line[x * 4:x * 4 + 4]))
    return width, height, pixels


def check(path):
    """校验磁盘产物与脚本输出一致，且满足模板图标的三项不变量。

    存在的意义：「产物可由脚本复现」这个承诺，不挂成可重复执行的检查就只活在注释里。
    模板图一旦 RGB 通道非 0 或尺寸变化，症状正是本次修复的原问题——深色菜单栏上不可见。
    """
    if not path.exists():
        print(f"FAIL: {path} 不存在", file=sys.stderr)
        return 1

    on_disk = path.read_bytes()
    expected = render_png_bytes(build_mask())

    problems = []
    if on_disk != expected:
        problems.append("产物与脚本输出不一致（脚本改了没重跑，或产物被直接编辑）")

    try:
        width, height, pixels = decode_rgba(on_disk)
    except Exception as exc:  # noqa: BLE001 - 解码失败本身就是要报告的结论
        print(f"FAIL: 无法解码 {path}：{exc}", file=sys.stderr)
        return 1

    if (width, height) != (SIZE, SIZE):
        problems.append(f"尺寸应为 {SIZE}x{SIZE}，实际 {width}x{height}")
    if any(px[0] or px[1] or px[2] for px in pixels):
        problems.append("RGB 通道存在非 0 值——模板图标的形状必须只由 alpha 表达")

    if problems:
        for p in problems:
            print(f"FAIL: {p}", file=sys.stderr)
        print(f"修复：重跑 python3 {Path(__file__).name} 并提交产物", file=sys.stderr)
        return 1

    print(f"OK: {path.name} 与脚本一致（{SIZE}x{SIZE} RGBA，RGB 恒 0，{len(pixels)} 像素）")
    return 0


def main():
    out = Path(__file__).resolve().parent.parent / "src-tauri" / "icons" / "tray-macos.png"

    if "--check" in sys.argv[1:]:
        return check(out)

    write_png(out, build_mask())
    print(f"wrote {out} ({SIZE}x{SIZE} RGBA, template icon)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
