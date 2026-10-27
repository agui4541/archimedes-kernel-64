/* SPDX-License-Identifier: GPL-2.0 */

/*
 * BQ25601 charger driver reconstructed from the Archimedes stock 32-bit
 * kernel.  The stock driver uses the MediaTek charger_device framework and
 * exposes this charger as primary_chg when charger_name is absent.
 */

#include <linux/delay.h>
#include <linux/err.h>
#include <linux/i2c.h>
#include <linux/init.h>
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/slab.h>

#include "charger_class.h"
#include "bq2560x_charger.h"

struct bq2560x_desc {
	u32 usb_vlim_mv;
	u32 usb_ilim_ma;
	u32 usb_vreg_mv;
	u32 usb_ichg_ma;
	u32 precharge_ma;
	u32 termination_ma;
	u32 boost_mv;
	u32 boost_ma;
	u32 vac_ovp_mv;
	u32 stat_pin_ctrl;
	const char *chg_dev_name;
};

struct bq2560x_chip {
	struct i2c_client *client;
	struct device *dev;
	struct bq2560x_desc desc;
	struct charger_device *chg_dev;
	struct charger_properties chg_props;
	struct mutex io_lock;
};

static int bq2560x_set_mivr(struct charger_device *chg_dev, u32 uv);

static int bq2560x_read_reg(struct bq2560x_chip *chip, u8 reg, u8 *val)
{
	int ret;

	mutex_lock(&chip->io_lock);
	ret = i2c_smbus_read_byte_data(chip->client, reg);
	if (ret >= 0)
		*val = ret & 0xff;
	mutex_unlock(&chip->io_lock);

	return ret < 0 ? ret : 0;
}

static int bq2560x_update_bits(struct bq2560x_chip *chip, u8 reg,
				       u8 mask, u8 val)
{
	int ret;
	u8 old, new;

	mutex_lock(&chip->io_lock);
	ret = i2c_smbus_read_byte_data(chip->client, reg);
	if (ret < 0)
		goto out;

	old = ret & 0xff;
	new = (old & ~mask) | (val & mask);
	if (new != old)
		ret = i2c_smbus_write_byte_data(chip->client, reg, new);
	else
		ret = 0;
out:
	mutex_unlock(&chip->io_lock);
	return ret;
}

static u8 bq2560x_encode_iinlim(u32 ua)
{
	if (ua >= BQ2560X_IINLIM_MAX_UA)
		return 0x1f;
	if (ua <= BQ2560X_IINLIM_MIN_UA)
		return 0;
	return (ua - BQ2560X_IINLIM_MIN_UA) /
		BQ2560X_IINLIM_STEP_UA;
}

static u32 bq2560x_decode_iinlim(u8 reg)
{
	return BQ2560X_IINLIM_MIN_UA +
		((reg & BQ2560X_INPUT_IINLIM_MASK) * BQ2560X_IINLIM_STEP_UA);
}

static u8 bq2560x_encode_ichg(u32 ua)
{
	return min_t(u32, ua / BQ2560X_ICHG_STEP_UA,
		BQ2560X_ICHG_MAX_UA / BQ2560X_ICHG_STEP_UA);
}

static u32 bq2560x_decode_ichg(u8 reg)
{
	return (reg & BQ2560X_CHARGE_ICHG_MASK) * BQ2560X_ICHG_STEP_UA;
}

static u8 bq2560x_encode_vreg(u32 uv)
{
	if (uv <= BQ2560X_VREG_MIN_UV)
		return 0;
	if (uv >= BQ2560X_VREG_MAX_UV)
		return (BQ2560X_VREG_MAX_UV - BQ2560X_VREG_MIN_UV) /
			BQ2560X_VREG_STEP_UV;
	return (uv - BQ2560X_VREG_MIN_UV) / BQ2560X_VREG_STEP_UV;
}

static u32 bq2560x_decode_vreg(u8 reg)
{
	u8 code = (reg & BQ2560X_VREG_MASK) >> 3;

	if (code == 0x0f)
		return 4352000;

	return BQ2560X_VREG_MIN_UV +
		(code * BQ2560X_VREG_STEP_UV);
}

static u8 bq2560x_encode_vindpm(u32 uv)
{
	if (uv <= BQ2560X_VINDPM_MIN_UV)
		return 0;
	if (uv >= BQ2560X_VINDPM_MAX_UV)
		return 0x0f;
	return (uv - BQ2560X_VINDPM_MIN_UV) /
		BQ2560X_VINDPM_STEP_UV;
}

static u8 bq2560x_encode_iterm(u32 ua)
{
	if (ua <= BQ2560X_ITERM_MIN_UA)
		return 0;
	if (ua >= BQ2560X_ITERM_MAX_UA)
		return 0x0f;
	return (ua - BQ2560X_ITERM_MIN_UA) / BQ2560X_ITERM_STEP_UA;
}

static int bq2560x_set_input_current(struct charger_device *chg_dev, u32 ua)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	u8 mask = BQ2560X_INPUT_IINLIM_MASK | BQ2560X_INPUT_HIZ_MASK;
	int ret;

	value = bq2560x_encode_iinlim(ua);
	if (!ua)
		value |= BQ2560X_INPUT_HIZ_MASK;

	ret = bq2560x_update_bits(chip, BQ2560X_REG_INPUT, mask, value);
	if (ret < 0)
		return ret;

	return bq2560x_set_mivr(chg_dev, 4500000);
}

static int bq2560x_get_input_current(struct charger_device *chg_dev, u32 *ua)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	int ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_INPUT, &value);
	if (!ret)
		*ua = value & BQ2560X_INPUT_HIZ_MASK ? 0 :
			bq2560x_decode_iinlim(value);
	return ret;
}

static int bq2560x_set_charging_current(struct charger_device *chg_dev,
					 u32 ua)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_CHARGE_CURRENT,
			BQ2560X_CHARGE_ICHG_MASK, bq2560x_encode_ichg(ua));
}

static int bq2560x_get_charging_current(struct charger_device *chg_dev,
					 u32 *ua)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	int ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_CHARGE_CURRENT, &value);
	if (!ret)
		*ua = bq2560x_decode_ichg(value);
	return ret;
}

static int bq2560x_set_constant_voltage(struct charger_device *chg_dev,
					 u32 uv)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_CHARGE_VOLTAGE,
			BQ2560X_VREG_MASK, bq2560x_encode_vreg(uv) << 3);
}

static int bq2560x_get_constant_voltage(struct charger_device *chg_dev,
					 u32 *uv)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	int ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_CHARGE_VOLTAGE, &value);
	if (!ret)
		*uv = bq2560x_decode_vreg(value);
	return ret;
}

static int bq2560x_set_mivr(struct charger_device *chg_dev, u32 uv)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_BOOST,
			BQ2560X_BOOST_VINDPM_MASK, bq2560x_encode_vindpm(uv));
}

static int bq2560x_enable(struct charger_device *chg_dev, bool enable)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_POWER,
			BQ2560X_POWER_OTG_CHG_MASK,
			enable ? BQ2560X_POWER_CHG_ENABLE : 0);
}

static int bq2560x_is_enabled(struct charger_device *chg_dev, bool *enable)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	int ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_POWER, &value);
	if (!ret)
		*enable = (value & BQ2560X_POWER_OTG_CHG_MASK) ==
			BQ2560X_POWER_CHG_ENABLE;
	return ret;
}

static int bq2560x_plug_in(struct charger_device *chg_dev)
{
	return bq2560x_enable(chg_dev, true);
}

static int bq2560x_plug_out(struct charger_device *chg_dev)
{
	return bq2560x_enable(chg_dev, false);
}

static int bq2560x_kick_wdt(struct charger_device *chg_dev)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_POWER,
			BQ2560X_POWER_WDT_RESET, BQ2560X_POWER_WDT_RESET);
}

static int bq2560x_disable_watchdog_timer(struct bq2560x_chip *chip)
{
	return bq2560x_update_bits(chip, BQ2560X_REG_TIMER,
			BQ2560X_TIMER_WDT_MASK, 0);
}

static int bq2560x_enable_safety_timer(struct charger_device *chg_dev,
					       bool enable)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_TIMER,
			BQ2560X_TIMER_ENABLE,
			enable ? BQ2560X_TIMER_ENABLE : 0);
}

static int bq2560x_is_safety_timer_enabled(struct charger_device *chg_dev,
						bool *enable)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	int ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_TIMER, &value);
	if (!ret)
		*enable = !!(value & BQ2560X_TIMER_ENABLE);
	return ret;
}

static int bq2560x_enable_termination(struct charger_device *chg_dev,
					      bool enable)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_TIMER,
			BQ2560X_TIMER_TERM_ENABLE,
			enable ? BQ2560X_TIMER_TERM_ENABLE : 0);
}

static int bq2560x_is_charging_done(struct charger_device *chg_dev,
					    bool *done)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	int ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_STATUS, &value);
	if (!ret)
		*done = ((value & BQ2560X_STATUS_CHARGE_MASK) >> 3) ==
			BQ2560X_STATUS_CHARGE_DONE;
	return ret;
}

static int bq2560x_set_eoc_current(struct charger_device *chg_dev, u32 ua)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_CHARGE_LIMIT,
			BQ2560X_LIMIT_ITERM_MASK, bq2560x_encode_iterm(ua));
}

static int bq2560x_set_recharge_voltage(struct charger_device *chg_dev,
						 u32 uv)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_CHARGE_VOLTAGE,
			BQ2560X_VRECHG_MASK,
			uv >= 200000 ? BQ2560X_VRECHG_MASK : 0);
}

static int bq2560x_get_eoc_current(struct charger_device *chg_dev, u32 *ua)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	u8 value;
	int ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_CHARGE_LIMIT, &value);
	if (!ret)
		*ua = BQ2560X_ITERM_MIN_UA +
			((value & BQ2560X_LIMIT_ITERM_MASK) *
			 BQ2560X_ITERM_STEP_UA);
	return ret;
}

static int bq2560x_enable_otg(struct charger_device *chg_dev, bool enable)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_POWER,
			BQ2560X_POWER_OTG_CHG_MASK,
			enable ? (2 << 4) : 0);
}

static int bq2560x_set_boost_current_limit(struct charger_device *chg_dev,
						 u32 ua)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);

	return bq2560x_update_bits(chip, BQ2560X_REG_CHARGE_CURRENT,
			BQ2560X_CHARGE_BOOST_LIM,
			ua == BQ2560X_BOOST_1P2A_UA ?
			BQ2560X_CHARGE_BOOST_LIM : 0);
}

static int bq2560x_set_boost_voltage(struct bq2560x_chip *chip, u32 mv)
{
	u8 value;

	if (mv <= 4850)
		value = 0;
	else if (mv <= 5000)
		value = 1 << 4;
	else if (mv <= 5150)
		value = 2 << 4;
	else
		value = 3 << 4;

	return bq2560x_update_bits(chip, BQ2560X_REG_BOOST,
			BQ2560X_BOOST_VOLTAGE_MASK, value);
}

static int bq2560x_set_vac_ovp(struct bq2560x_chip *chip, u32 mv)
{
	u8 value;

	if (mv >= 14300)
		value = 3 << 6;
	else if (mv >= 10500)
		value = 2 << 6;
	else if (mv >= 6200)
		value = 1 << 6;
	else
		value = 0;

	return bq2560x_update_bits(chip, BQ2560X_REG_BOOST,
			BQ2560X_BOOST_OVP_MASK, value);
}

static int bq2560x_dump_registers(struct charger_device *chg_dev)
{
	struct bq2560x_chip *chip = charger_get_data(chg_dev);
	int i, ret;
	u8 value;

	for (i = 0; i <= BQ2560X_REG_DEVICE; i++) {
		ret = bq2560x_read_reg(chip, i, &value);
		if (ret < 0)
			return ret;
		dev_info(chip->dev, "REG_0x%02x=0x%02x\n", i, value);
	}
	return 0;
}

static const struct charger_ops bq2560x_chg_ops = {
	.enable = bq2560x_enable,
	.is_enabled = bq2560x_is_enabled,
	.plug_in = bq2560x_plug_in,
	.plug_out = bq2560x_plug_out,
	.get_charging_current = bq2560x_get_charging_current,
	.set_charging_current = bq2560x_set_charging_current,
	.get_input_current = bq2560x_get_input_current,
	.set_input_current = bq2560x_set_input_current,
	.get_eoc_current = bq2560x_get_eoc_current,
	.set_eoc_current = bq2560x_set_eoc_current,
	.get_constant_voltage = bq2560x_get_constant_voltage,
	.set_constant_voltage = bq2560x_set_constant_voltage,
	.kick_wdt = bq2560x_kick_wdt,
	.set_mivr = bq2560x_set_mivr,
	.enable_safety_timer = bq2560x_enable_safety_timer,
	.is_safety_timer_enabled = bq2560x_is_safety_timer_enabled,
	.enable_termination = bq2560x_enable_termination,
	.is_charging_done = bq2560x_is_charging_done,
	.enable_otg = bq2560x_enable_otg,
	.set_boost_current_limit = bq2560x_set_boost_current_limit,
	.dump_registers = bq2560x_dump_registers,
};

static int bq2560x_parse_dt(struct bq2560x_chip *chip)
{
	struct device_node *np = chip->dev->of_node;
	struct bq2560x_desc *desc = &chip->desc;
	const char *name;
	const char *alias;

	if (!np)
		return -ENODEV;

	/* The stock DT stores charger voltage/current values in mV/mA. */
	of_property_read_u32(np, "ti,bq2560x,usb-vlim", &desc->usb_vlim_mv);
	of_property_read_u32(np, "ti,bq2560x,usb-ilim", &desc->usb_ilim_ma);
	of_property_read_u32(np, "ti,bq2560x,usb-vreg", &desc->usb_vreg_mv);
	of_property_read_u32(np, "ti,bq2560x,usb-ichg", &desc->usb_ichg_ma);
	of_property_read_u32(np, "ti,bq2560x,precharge-current",
			     &desc->precharge_ma);
	of_property_read_u32(np, "ti,bq2560x,termination-current",
			     &desc->termination_ma);
	of_property_read_u32(np, "ti,bq2560x,boost-voltage",
			     &desc->boost_mv);
	of_property_read_u32(np, "ti,bq2560x,boost-current",
			     &desc->boost_ma);
	of_property_read_u32(np, "ti,bq2560x,vac-ovp-threshold",
			     &desc->vac_ovp_mv);
	of_property_read_u32(np, "ti,bq2560x,stat-pin-ctrl",
			     &desc->stat_pin_ctrl);

	if (!desc->usb_vlim_mv)
		desc->usb_vlim_mv = 4600;
	if (!desc->usb_ilim_ma)
		desc->usb_ilim_ma = 500;
	if (!desc->usb_vreg_mv)
		desc->usb_vreg_mv = 4400;
	if (!desc->usb_ichg_ma)
		desc->usb_ichg_ma = 500;
	if (!desc->precharge_ma)
		desc->precharge_ma = 256;
	if (!desc->termination_ma)
		desc->termination_ma = 192;
	if (!desc->boost_mv)
		desc->boost_mv = 5000;
	if (!desc->boost_ma)
		desc->boost_ma = 1200;
	if (!desc->vac_ovp_mv)
		desc->vac_ovp_mv = 6200;

	if (of_property_read_string(np, "charger_name", &name) < 0)
		name = "primary_chg";
	desc->chg_dev_name = name;

	if (of_property_read_string(np, "alias_name", &alias) < 0)
		alias = "bq2560x";
	chip->chg_props.alias_name = alias;
	return 0;
}

static u8 bq2560x_encode_precharge(u32 ma)
{
	if (ma <= 60)
		return 0;
	if (ma >= 960)
		return 0xf;
	return (ma - 60) / 60;
}

static int bq2560x_init_device(struct bq2560x_chip *chip)
{
	struct bq2560x_desc *desc = &chip->desc;
	int ret;

	ret = bq2560x_disable_watchdog_timer(chip);
	if (ret < 0)
		return ret;
	ret = bq2560x_update_bits(chip, BQ2560X_REG_INPUT,
			BQ2560X_INPUT_STAT_MASK, desc->stat_pin_ctrl << 5);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_boost_voltage(chip, desc->boost_mv);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_vac_ovp(chip, desc->vac_ovp_mv);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_boost_current_limit(chip->chg_dev,
			desc->boost_ma * 1000);
	if (ret < 0)
		return ret;
	ret = bq2560x_update_bits(chip, BQ2560X_REG_CHARGE_LIMIT,
			BQ2560X_LIMIT_IPRECHG_MASK,
			bq2560x_encode_precharge(desc->precharge_ma) << 4);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_eoc_current(chip->chg_dev,
			desc->termination_ma * 1000);
	if (ret < 0)
		return ret;

	/* Match the stock BQ2560x hardware-init sequence. */
	ret = bq2560x_set_mivr(chip->chg_dev, 4500000);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_eoc_current(chip->chg_dev, 150000);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_constant_voltage(chip->chg_dev, 4350000);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_input_current(chip->chg_dev, 500000);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_charging_current(chip->chg_dev, 500000);
	if (ret < 0)
		return ret;
	ret = bq2560x_set_recharge_voltage(chip->chg_dev, 100000);
	if (ret < 0)
		return ret;
	ret = bq2560x_enable_safety_timer(chip->chg_dev, true);
	if (ret < 0)
		return ret;
	ret = bq2560x_enable_termination(chip->chg_dev, true);
	if (ret < 0)
		return ret;
	ret = bq2560x_enable(chip->chg_dev, true);
	if (ret < 0)
		return ret;

	return 0;
}

static int bq2560x_probe(struct i2c_client *client,
				const struct i2c_device_id *id)
{
	struct bq2560x_chip *chip;
	int ret;
	u8 device;

	chip = devm_kzalloc(&client->dev, sizeof(*chip), GFP_KERNEL);
	if (!chip)
		return -ENOMEM;

	chip->client = client;
	chip->dev = &client->dev;
	mutex_init(&chip->io_lock);
	i2c_set_clientdata(client, chip);

	ret = bq2560x_parse_dt(chip);
	if (ret < 0)
		return ret;

	ret = bq2560x_read_reg(chip, BQ2560X_REG_DEVICE, &device);
	if (ret < 0)
		return ret;
	if (((device & BQ2560X_DEVICE_PN_MASK) >> 3) != BQ25601_DEVICE_PN)
		dev_warn(chip->dev, "unexpected BQ2560x part 0x%02x\n", device);

	chip->chg_dev = charger_device_register(chip->desc.chg_dev_name,
			&client->dev, chip, &bq2560x_chg_ops, &chip->chg_props);
	if (IS_ERR(chip->chg_dev))
		return PTR_ERR(chip->chg_dev);
	if (!chip->chg_dev)
		return -EINVAL;

	ret = bq2560x_init_device(chip);
	if (ret < 0) {
		charger_device_unregister(chip->chg_dev);
		return ret;
	}

	bq2560x_dump_registers(chip->chg_dev);
	dev_info(chip->dev, "BQ2560x registered as %s\n",
		chip->desc.chg_dev_name);
	return 0;
}

static int bq2560x_remove(struct i2c_client *client)
{
	struct bq2560x_chip *chip = i2c_get_clientdata(client);

	if (chip->chg_dev)
		charger_device_unregister(chip->chg_dev);
	return 0;
}

static const struct i2c_device_id bq2560x_i2c_id[] = {
	{ "bq2560x-charger", 0 },
	{ "bq25601-charger", 0 },
	{ "bq2560x", 0 },
	{ }
};
MODULE_DEVICE_TABLE(i2c, bq2560x_i2c_id);

static const struct of_device_id bq2560x_of_match[] = {
	{ .compatible = "ti,bq25601" },
	{ .compatible = "ti,bq2560x_charger" },
	{ }
};
MODULE_DEVICE_TABLE(of, bq2560x_of_match);

static struct i2c_driver bq2560x_driver = {
	.driver = {
		.name = "bq25601-charger",
		.of_match_table = bq2560x_of_match,
	},
	.probe = bq2560x_probe,
	.remove = bq2560x_remove,
	.id_table = bq2560x_i2c_id,
};

module_i2c_driver(bq2560x_driver);

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("MediaTek BQ25601 charger driver");
MODULE_AUTHOR("Archimedes community / reconstructed from stock kernel");
