# 读产品自己的镜像信息命令（BackupRestore.exe wim-info <镜像> --json）的原样输出，回答「这个镜像里有几个索引、
# 多大、是哪个版本的程序写出来的」。
# 被 tools/vm/gui-backup.sh 调用；tools/vm/selftest.sh 用命令行冒烟留下的真实样本离线验证它。
#
# 用法：python3 judge/wim-info.py <输出文件> [期望的程序版本]
# 输出（一行一个键值）：
#   WIM_INFO_READ=True|False        读到了一行 JSON 结果没有（读不到不等于没有镜像，也不是镜像坏了）
#   WIM_INFO_SUCCESS=True|False     命令自陈成功
#   WIM_IMAGE_COUNT=<n>             镜像里的索引个数
#   WIM_INDEX1_SIZE=<bytes>         第 1 个索引的镜像大小（副档里记的；读不到输出 <missing>）
#   WIM_PROGRAM_VERSION=<版本>      写出这个镜像的程序版本（副档里记的；缺字段输出 <missing>）
#   WIM_VERSION_MATCH=True|False    与期望版本一致（没给期望版本时输出 <not requested>）
import json
import sys


def judge(path, expected=None, out=print):
    try:
        raw = open(path, "rb").read().decode("utf-8-sig", "replace")
    except OSError:
        out("WIM_INFO_READ=False reason=cannot_read_file")
        return False
    line = next((l for l in raw.splitlines() if l.strip().startswith("{")), "")
    try:
        value = json.loads(line)
    except ValueError:
        out("WIM_INFO_READ=False reason=no_json_line")
        return False
    images = value.get("images")
    if not isinstance(images, list):
        out("WIM_INFO_READ=False reason=no_images_field")
        return False
    out("WIM_INFO_READ=True")
    # 「自陈成功」在输出的第二行 JSON 里（命令的结论行）；第一行是结果对象，没有这个字段。
    success = value.get("success")
    for other in raw.splitlines():
        if success is not None:
            break
        if other.strip().startswith("{"):
            try:
                success = json.loads(other).get("success")
            except ValueError:
                pass
    out("WIM_INFO_SUCCESS=%s" % ("<missing>" if success is None else success))
    out("WIM_IMAGE_COUNT=%d" % len(images))
    first = images[0] if images and isinstance(images[0], dict) else {}
    size = first.get("imageSize", first.get("ImageSize"))
    out("WIM_INDEX1_SIZE=%s" % ("<missing>" if size is None else size))
    version = first.get("programVersion", first.get("ProgramVersion"))
    out("WIM_PROGRAM_VERSION=%s" % ("<missing>" if version is None else version))
    if expected is None:
        out("WIM_VERSION_MATCH=<not requested>")
    else:
        out("WIM_VERSION_MATCH=%s" % (version == expected))
    return True


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        sys.exit("usage: judge/wim-info.py <output file> [expected program version]")
    ok = judge(sys.argv[1], sys.argv[2] if len(sys.argv) == 3 else None)
    sys.exit(0 if ok else 1)
