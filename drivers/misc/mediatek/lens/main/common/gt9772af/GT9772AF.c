/*
 * Giantec GT9772AF voice coil motor driver for the legacy MediaTek
 * main-lens framework.
 *
 * GT9772 exposes a 10-bit DAC at registers 0x03 (MSB) and 0x04 (LSB).
 * The retail MediaTek driver writes those registers as two separate I2C
 * transactions and enables the device's advance mode with 0xED=0xAB.
 */
#include <linux/delay.h>
#include <linux/fs.h>
#include <linux/i2c.h>
#include <linux/uaccess.h>

#include "lens_info.h"

#define AF_DRVNAME "GT9772AF_DRV"
#define AF_I2C_SLAVE_ADDR 0x18
#define GT9772AF_MAX_POSITION 1023
#define GT9772AF_ADVANCE_REG 0xed
#define GT9772AF_ADVANCE_VALUE 0xab

#define LOG_INF(format, args...) \
	pr_info(AF_DRVNAME " [%s] " format, __func__, ##args)

static struct i2c_client *g_pstAF_I2Cclient;
static int *g_pAF_Opened;
static spinlock_t *g_pAF_SpinLock;
static unsigned long g_u4AF_INF;
static unsigned long g_u4AF_MACRO = GT9772AF_MAX_POSITION;
static unsigned long g_u4CurrPosition;

static void gt9772af_select_client(void)
{
	g_pstAF_I2Cclient->addr = AF_I2C_SLAVE_ADDR >> 1;
}

static int gt9772af_read_reg(u8 reg, u8 *value)
{
	int ret;
	char cmd = reg;

	gt9772af_select_client();
	ret = i2c_master_send(g_pstAF_I2Cclient, &cmd, 1);
	if (ret < 0)
		return ret;
	ret = i2c_master_recv(g_pstAF_I2Cclient, value, 1);
	return ret < 0 ? ret : 0;
}

static int gt9772af_write_reg(u8 reg, u8 value)
{
	char cmd[2] = { reg, value };

	gt9772af_select_client();
	return i2c_master_send(g_pstAF_I2Cclient, cmd, 2) == 2 ? 0 : -EIO;
}

static int gt9772af_write_position(unsigned long position)
{
	u8 msb;
	u8 lsb;
	int ret;

	if (position > GT9772AF_MAX_POSITION)
		position = GT9772AF_MAX_POSITION;

	/* The factory driver sends MSB and LSB as separate register writes. */
	msb = (position >> 8) & 0x03;
	lsb = position & 0xff;
	ret = gt9772af_write_reg(0x03, msb);
	if (ret)
		return ret;
	ret = gt9772af_write_reg(0x04, lsb);
	if (ret)
		return ret;
	return 0;
}

static inline int getAFInfo(__user struct stAF_MotorInfo *user_info)
{
	struct stAF_MotorInfo info;

	info.u4MacroPosition = g_u4AF_MACRO;
	info.u4InfPosition = g_u4AF_INF;
	info.u4CurrentPosition = g_u4CurrPosition;
	info.bIsSupportSR = 1;
	info.bIsMotorMoving = 1;
	info.bIsMotorOpen = *g_pAF_Opened >= 1;

	if (copy_to_user(user_info, &info, sizeof(info)))
		return -EFAULT;
	return 0;
}

static int initAF(void)
{
	u8 id = 0xff;
	int ret;

	if (*g_pAF_Opened != 1)
		return 0;

	/* Match the retail driver: enter the actuator's advance mode. */
	ret = gt9772af_write_reg(GT9772AF_ADVANCE_REG,
		GT9772AF_ADVANCE_VALUE);
	if (ret)
		return ret;
	if (!gt9772af_read_reg(0x00, &id))
		LOG_INF("chip id 0x%02x\n", id);

	spin_lock(g_pAF_SpinLock);
	*g_pAF_Opened = 2;
	spin_unlock(g_pAF_SpinLock);
	LOG_INF("driver init success\n");
	return 0;
}

static inline int moveAF(unsigned long position)
{
	int ret = gt9772af_write_position(position);

	if (!ret) {
		g_u4CurrPosition = position > GT9772AF_MAX_POSITION ?
			GT9772AF_MAX_POSITION : position;
		LOG_INF("move position %lu\n", g_u4CurrPosition);
	} else {
		LOG_INF("set position failed: %d\n", ret);
	}
	return ret;
}

static inline int setAFInf(unsigned long position)
{
	spin_lock(g_pAF_SpinLock);
	g_u4AF_INF = position;
	spin_unlock(g_pAF_SpinLock);
	return 0;
}

static inline int setAFMacro(unsigned long position)
{
	spin_lock(g_pAF_SpinLock);
	g_u4AF_MACRO = position > GT9772AF_MAX_POSITION ?
		GT9772AF_MAX_POSITION : position;
	spin_unlock(g_pAF_SpinLock);
	return 0;
}

long GT9772AF_Ioctl(struct file *file, unsigned int command,
			unsigned long param)
{
	switch (command) {
	case AFIOC_G_MOTORINFO:
		return getAFInfo((__user struct stAF_MotorInfo *)param);
	case AFIOC_T_MOVETO:
		return moveAF(param);
	case AFIOC_T_SETINFPOS:
		return setAFInf(param);
	case AFIOC_T_SETMACROPOS:
		return setAFMacro(param);
	default:
		return -EPERM;
	}
}

int GT9772AF_Release(struct inode *inode, struct file *file)
{
	if (*g_pAF_Opened) {
		spin_lock(g_pAF_SpinLock);
		*g_pAF_Opened = 0;
		spin_unlock(g_pAF_SpinLock);
	}
	return 0;
}

int GT9772AF_SetI2Cclient(struct i2c_client *client,
			  spinlock_t *lock, int *opened)
{
	g_pstAF_I2Cclient = client;
	g_pAF_SpinLock = lock;
	g_pAF_Opened = opened;
	return initAF() ? 0 : 1;
}

int GT9772AF_GetFileName(unsigned char *name)
{
	name[0] = '\0';
	return 1;
}
