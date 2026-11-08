// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * predator_wmi - minimal ACPI-WMI pass-through for Acer Predator laptops.
 *
 * On Windows, PredatorSense talks to the firmware through the generic WMI
 * service: it calls methods of the classes AcerGamingFunction, APGeAction and
 * BatteryControl with already-encoded integers/byte arrays. This driver gives
 * the Linux daemon exactly that and nothing more: it binds to those three WMI
 * GUIDs and lets root evaluate a fixed allow-list of their methods through
 * /dev/predator-wmi. No feature logic and no EC access live here.
 *
 * Hotkeys/events stay with acer-wmi, which does not bind these GUIDs.
 */
#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/acpi.h>
#include <linux/bits.h>
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <linux/version.h>
#include <linux/wmi.h>

#include "predator_wmi_uapi.h"

#define GUID_GAMING  "7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56"
#define GUID_APGE    "61EF69EA-865C-4BC3-A502-A0DEBA0CB531"
#define GUID_BATTERY "79772EC5-04B1-4BFD-843C-61E7F77B6CC9"

/*
 * Methods userspace may call, per interface (bit n = WmiMethodId n).
 * IDs come from the firmware's embedded MOF (see docs/REVERSE_ENGINEERING.md).
 */
static const u64 allowed_methods[PREDATOR_WMI_IFACE_COUNT] = {
	[PREDATOR_WMI_IFACE_GAMING] =
		BIT_ULL(1)  | /* SetGamingProfile */
		BIT_ULL(2)  | /* SetGamingLED */
		BIT_ULL(3)  | /* GetGamingProfile */
		BIT_ULL(4)  | /* GetGamingLED */
		BIT_ULL(5)  | /* GetGamingSysInfo */
		BIT_ULL(6)  | /* SetGamingRgbKb */
		BIT_ULL(7)  | /* GetGamingRgbKb */
		BIT_ULL(12) | /* SetBackLogo (rear lid logo, PHN16-72) */
		BIT_ULL(13) | /* GetBackLogo */
		BIT_ULL(14) | /* SetGamingFanBehavior */
		BIT_ULL(15) | /* GetGamingFanBehavior */
		BIT_ULL(16) | /* SetGamingFanSpeed */
		BIT_ULL(17) | /* GetGamingFanSpeed */
		BIT_ULL(20) | /* SetGamingKBBacklight */
		BIT_ULL(21) | /* GetGamingKBBacklight */
		BIT_ULL(22) | /* SetGamingMiscSetting */
		BIT_ULL(23),  /* GetGamingMiscSetting */
	[PREDATOR_WMI_IFACE_APGE] =
		BIT_ULL(1)  | /* SetFunction */
		BIT_ULL(2),   /* GetFunction */
	[PREDATOR_WMI_IFACE_BATTERY] =
		BIT_ULL(19) | /* GetBatteryInformation (read-only) */
		BIT_ULL(20) | /* GetBatteryHealthControlStatus */
		BIT_ULL(21),  /* SetBatteryHealthControl */
};

static struct wmi_device *ifaces[PREDATOR_WMI_IFACE_COUNT];
static DEFINE_MUTEX(predator_lock); /* protects ifaces[] and serializes calls */

static int predator_do_call(struct predator_wmi_call *c)
{
	struct acpi_buffer in, out = { ACPI_ALLOCATE_BUFFER, NULL };
	union acpi_object *obj;
	acpi_status status;
	int ret = 0;

	if (c->iface >= PREDATOR_WMI_IFACE_COUNT || c->reserved ||
	    c->in_len > PREDATOR_WMI_MAX_IN || c->method_id >= 64 ||
	    !(allowed_methods[c->iface] & BIT_ULL(c->method_id)))
		return -EINVAL;

	in.length = c->in_len;
	in.pointer = c->in;

	mutex_lock(&predator_lock);
	if (!ifaces[c->iface]) {
		mutex_unlock(&predator_lock);
		return -ENODEV;
	}
	status = wmidev_evaluate_method(ifaces[c->iface], 0, c->method_id, &in, &out);
	mutex_unlock(&predator_lock);

	if (ACPI_FAILURE(status)) {
		pr_debug("iface %u method %u failed: %s\n", c->iface, c->method_id,
			 acpi_format_exception(status));
		return -EIO;
	}

	c->out_len = 0;
	c->out_type = PREDATOR_WMI_OUT_NONE;
	memset(c->out, 0, sizeof(c->out));

	obj = out.pointer;
	if (!obj)
		return 0;

	switch (obj->type) {
	case ACPI_TYPE_INTEGER: {
		__le64 v = cpu_to_le64(obj->integer.value);

		memcpy(c->out, &v, sizeof(v));
		c->out_len = sizeof(v);
		c->out_type = PREDATOR_WMI_OUT_INTEGER;
		break;
	}
	case ACPI_TYPE_BUFFER:
		c->out_len = min_t(u32, obj->buffer.length, PREDATOR_WMI_MAX_OUT);
		memcpy(c->out, obj->buffer.pointer, c->out_len);
		c->out_type = PREDATOR_WMI_OUT_BUFFER;
		break;
	default:
		ret = -EPROTO;
		break;
	}
	kfree(obj);
	return ret;
}

static long predator_ioctl(struct file *f, unsigned int cmd, unsigned long arg)
{
	void __user *uarg = (void __user *)arg;

	switch (cmd) {
	case PREDATOR_WMI_IOC_INFO: {
		struct predator_wmi_info info = { .api_version = PREDATOR_WMI_API_VERSION };
		int i;

		mutex_lock(&predator_lock);
		for (i = 0; i < PREDATOR_WMI_IFACE_COUNT; i++)
			if (ifaces[i])
				info.iface_mask |= BIT(i);
		mutex_unlock(&predator_lock);
		return copy_to_user(uarg, &info, sizeof(info)) ? -EFAULT : 0;
	}
	case PREDATOR_WMI_IOC_CALL: {
		struct predator_wmi_call *c;
		int ret;

		c = kzalloc(sizeof(*c), GFP_KERNEL);
		if (!c)
			return -ENOMEM;
		if (copy_from_user(c, uarg, sizeof(*c))) {
			kfree(c);
			return -EFAULT;
		}
		ret = predator_do_call(c);
		if (!ret && copy_to_user(uarg, c, sizeof(*c)))
			ret = -EFAULT;
		kfree(c);
		return ret;
	}
	default:
		return -ENOTTY;
	}
}

static int predator_open(struct inode *inode, struct file *f)
{
	/* Firmware control is a privileged operation, like on Windows. */
	if (!capable(CAP_SYS_ADMIN))
		return -EPERM;
	return nonseekable_open(inode, f);
}

static const struct file_operations predator_fops = {
	.owner          = THIS_MODULE,
	.open           = predator_open,
	.unlocked_ioctl = predator_ioctl,
	.compat_ioctl   = compat_ptr_ioctl,
};

static struct miscdevice predator_misc = {
	.minor = MISC_DYNAMIC_MINOR,
	.name  = "predator-wmi",
	.fops  = &predator_fops,
	.mode  = 0600,
};

static int predator_probe(struct wmi_device *wdev, const void *context)
{
	unsigned long idx = (unsigned long)context;

	if (idx >= PREDATOR_WMI_IFACE_COUNT)
		return -EINVAL;

	mutex_lock(&predator_lock);
	ifaces[idx] = wdev;
	mutex_unlock(&predator_lock);
	dev_info(&wdev->dev, "bound interface %lu\n", idx);
	return 0;
}

static void predator_remove(struct wmi_device *wdev)
{
	int i;

	mutex_lock(&predator_lock);
	for (i = 0; i < PREDATOR_WMI_IFACE_COUNT; i++)
		if (ifaces[i] == wdev)
			ifaces[i] = NULL;
	mutex_unlock(&predator_lock);
}

static const struct wmi_device_id predator_wmi_ids[] = {
	{ .guid_string = GUID_GAMING,  .context = (void *)PREDATOR_WMI_IFACE_GAMING },
	{ .guid_string = GUID_APGE,    .context = (void *)PREDATOR_WMI_IFACE_APGE },
	{ .guid_string = GUID_BATTERY, .context = (void *)PREDATOR_WMI_IFACE_BATTERY },
	{ }
};
MODULE_DEVICE_TABLE(wmi, predator_wmi_ids);

static struct wmi_driver predator_wmi_driver = {
	.driver = {
		.name = "predator-wmi",
	},
	.id_table    = predator_wmi_ids,
	.probe       = predator_probe,
	.remove      = predator_remove,
#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 9, 0)
	.no_singleton = true,
#endif
};

static int __init predator_init(void)
{
	int ret;

	ret = misc_register(&predator_misc);
	if (ret)
		return ret;

	ret = wmi_driver_register(&predator_wmi_driver);
	if (ret)
		misc_deregister(&predator_misc);
	return ret;
}

static void __exit predator_exit(void)
{
	wmi_driver_unregister(&predator_wmi_driver);
	misc_deregister(&predator_misc);
}

module_init(predator_init);
module_exit(predator_exit);

MODULE_DESCRIPTION("ACPI-WMI pass-through for the Predator Sense Linux daemon");
MODULE_LICENSE("GPL");
