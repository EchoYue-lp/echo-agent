#!/usr/bin/env python3
"""小红书配图生成器 - 根据文章内容生成配图"""
import sys
import os
import random
import platform
from pathlib import Path
from PIL import Image, ImageFont, ImageDraw

# 默认参数
TITLE = ""
SUBTITLE = ""
CONTENT = ""

# 解析参数
if len(sys.argv) > 1:
    TITLE = sys.argv[1]
if len(sys.argv) > 2:
    SUBTITLE = sys.argv[2]
if len(sys.argv) > 3:
    CONTENT = sys.argv[3].replace("\\n", "\n")

# 配置
WIDTH, HEIGHT = 1080, 1440
BG_COLOR = (255, 250, 245)  # 暖白色


def _default_output_path():
    """跨平台输出路径：优先使用调用者当前目录"""
    return str(Path.cwd() / "xhs_cover.jpg")


OUTPUT_PATH = _default_output_path()


def _find_cjk_font():
    """跨平台查找 CJK 字体，返回可用的字体路径列表"""
    system = platform.system()
    candidates = []

    if system == "Darwin":  # macOS
        candidates = [
            "/System/Library/Fonts/STHeiti Medium.ttc",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Supplemental/Songti.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
    elif system == "Linux":
        candidates = [
            "/usr/share/fonts/opentype/noto/NotoSerifCJK-Bold.ttc",
            "/usr/share/fonts/noto-cjk/NotoSerifCJK-Bold.ttc",
            "/usr/share/fonts/truetype/noto/NotoSerifCJK-Bold.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ]
    elif system == "Windows":
        font_dir = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts")
        candidates = [
            os.path.join(font_dir, "msyh.ttc"),      # 微软雅黑
            os.path.join(font_dir, "simhei.ttf"),     # 黑体
            os.path.join(font_dir, "simsun.ttc"),     # 宋体
        ]

    for path in candidates:
        if os.path.isfile(path):
            return path

    return None


FONT_PATH = _find_cjk_font()

def generate_cover(title, subtitle, content, output_path=OUTPUT_PATH):
    """生成小红书配图"""
    
    # 创建图片
    img = Image.new('RGB', (WIDTH, HEIGHT), BG_COLOR)
    draw = ImageDraw.Draw(img)
    
    # 装饰元素 - 星星和圆点
    for _ in range(15):
        x = random.randint(0, WIDTH)
        y = random.randint(0, HEIGHT)
        r = random.randint(2, 8)
        draw.ellipse([x-r, y-r, x+r, y+r], fill=(255, 200, 100))
    
    # 加载字体
    if FONT_PATH:
        try:
            title_font = ImageFont.truetype(FONT_PATH, 52)
            content_font = ImageFont.truetype(FONT_PATH, 32)
        except Exception as e:
            print(f"字体加载失败: {e}，使用默认字体")
            title_font = ImageFont.load_default(size=40)
            content_font = ImageFont.load_default(size=24)
    else:
        print("未找到 CJK 字体，使用默认字体（中文可能无法正常显示）")
        title_font = ImageFont.load_default(size=40)
        content_font = ImageFont.load_default(size=24)
    
    # 绘制标题
    if title:
        draw.text((60, 180), title, fill=(60, 60, 60), font=title_font)
    
    # 绘制副标题
    if subtitle:
        draw.text((60, 250), subtitle, fill=(100, 100, 100), font=content_font)
    
    # 绘制内容
    if content:
        lines = content.split('\n')
        y_offset = 400
        for line in lines:
            if line.strip():
                draw.text((60, y_offset), line, fill=(80, 80, 80), font=content_font)
                y_offset += 50
    
    # 底部标签
    draw.text((60, HEIGHT - 100), "#今日分享", fill=(150, 150, 150), font=content_font)
    
    # 保存
    img.save(output_path, "JPEG", quality=95)
    print(f"配图已生成: {output_path}")
    return output_path

if __name__ == "__main__":
    generate_cover(TITLE, SUBTITLE, CONTENT)
