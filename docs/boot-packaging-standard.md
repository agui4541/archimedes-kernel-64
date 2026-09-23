# Archimedes MT6761 boot 打包标准

这台设备使用 MTK Android boot image v1。打包结果必须以“最后一版能启动的
display/USB-fix boot”为模板；不能用旧 touch DTB、不能只替换壳，也不能手工
猜地址或 `KERNEL_SZ`。

唯一脚本：`tools/package_boot_mtk.py`

```sh
python3 tools/package_boot_mtk.py \
  --good-boot boot-arm64-display-usbfix-32MiB.img \
  --ksu-kernel out/arch/arm64/boot/Image.gz \
  --output boot-kernelsu-display-usbfix-32MiB.img
```

脚本从 good boot 继承 header 和末尾 FDT，并从 KSU boot 的 `KERNEL_SZ` 中
自动剥离旧 FDT，再拼接新内核 + good FDT。成功条件：

- Android boot header v1（`header_size=1648`）、page size 2048；
- `kernel_addr=0x40080000`、`tags_addr=0x47880000`；
- cmdline 包含且保持 `bootopt=64S3,32S1,64S1`；
- good FDT 必须包含 `aw87329_pa`，并且 FDT totalsize 必须等于 payload 尾部；
- 重新计算 boot image ID（SHA-1）并清零剩余 ID 字节；
- 输出严格为 `33554432` 字节（32 MiB）；
- 输出再次解析出的 FDT 必须与 good FDT 完全一致。

刷机流程的固定顺序：先用 root ADB 备份 expdb，再清零 expdb，写入 boot，
回读并校验 SHA256；只有用户明确批准后才重启。出现黑屏/重启时，优先保留
这次干净 expdb，再恢复已知可启动 boot，禁止用旧日志覆盖现场。
