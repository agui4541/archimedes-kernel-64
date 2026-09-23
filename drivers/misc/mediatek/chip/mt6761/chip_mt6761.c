#include <linux/module.h>
#include <linux/errno.h>

unsigned int mt_get_chip_sw_ver(void) { return 0; }
EXPORT_SYMBOL(mt_get_chip_sw_ver);

int mt_dfs_general_pll(unsigned int pll_id, unsigned int dds)
{
    return -ENODEV;
}
EXPORT_SYMBOL(mt_dfs_general_pll);

__weak int register_ccci_sys_call_back(int md_id, unsigned int id, int (*func)(int, int)) { return 0; }
EXPORT_SYMBOL(register_ccci_sys_call_back);

/* Empty MT6761 platform chip object; common implementation supplies generic hooks. */
