# Archimedes MT6761 boot 打包标准

这台设备使用 MTK Android boot image v1。打包基线必须是最后一版已验证能启动且 Wi‑Fi/蓝牙正常的 boot：
`boot-wifi-emi-hole-fallback-20260927.img`。manager744 只能用于救砖，不能作为新镜像的 DTB 基线。

唯一脚本：`tools/package_boot_mtk.py`

```sh
python3 tools/package_boot_mtk.py \
  --good-boot boot-wifi-emi-hole-fallback-20260927.img \
  --ksu-kernel out/arch/arm64/boot/Image.gz \
  --output boot-kernelsu-wifi-bt-32MiB.img
```

脚本从 good boot 继承 header 和完整 DTB 尾部。该 MTK 镜像的尾部可能包含两个连续 FDT，必须整体保留；脚本会自动识别完整 DTB suffix，而不是只取最后一个 FDT。`--ksu-kernel` 只接受纯 `Image.gz`，明确拒绝 `Image.gz-dtb`，避免重复追加 DTB。

需要修改设备树时，先从 good boot 解出完整 DTB suffix，只对目标 FDT 做最小 `fdtput` 修改，再用 `--dtb` 传回完整 suffix；不要把源码全量编译出的 `mt6761.dtb` 替换进 boot，也不要把 manager744 的旧 DTB 混入。这样显示、电源、Wi‑Fi/蓝牙 reserved-memory 和厂商节点保持不变。

成功条件：

- Android boot header v1（`header_size=1648`）、page size 2048；
- `kernel_addr=0x40080000`、`tags_addr=0x47880000`；
- cmdline 包含且保持 `bootopt=64S3,32S1,64S1`；
- good DTB suffix 必须包含 `aw87329_pa`，且每个 FDT 的 totalsize 连续到 payload 尾部；
- 重新计算 boot image ID（SHA-1）并清零剩余 ID 字节；
- 输出严格为 `33554432` 字节（32 MiB）；
- 输出再次解析出的完整 DTB suffix 必须与选定 DTB 完全一致。

刷机流程的固定顺序：先用 root ADB 备份 expdb，再清零 expdb，写入 boot，回读并校验 SHA256；只有用户明确批准后才重启。出现黑屏/重启时，优先保留这次干净 expdb，再恢复已知可启动 boot，禁止用旧日志覆盖现场。
