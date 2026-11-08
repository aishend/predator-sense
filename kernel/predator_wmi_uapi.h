/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
/*
 * Userspace API of the predator_wmi pass-through driver.
 *
 * The driver exposes /dev/predator-wmi. Each ioctl call evaluates one ACPI-WMI
 * method of one of the Acer interfaces, the same way the Windows WMI service
 * does for PredatorSense. All feature logic lives in userspace.
 */
#ifndef _UAPI_PREDATOR_WMI_H
#define _UAPI_PREDATOR_WMI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define PREDATOR_WMI_API_VERSION 1

/* Interfaces (one ACPI-WMI GUID each) */
enum predator_wmi_iface {
	PREDATOR_WMI_IFACE_GAMING  = 0, /* AcerGamingFunction 7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56 */
	PREDATOR_WMI_IFACE_APGE    = 1, /* APGeAction         61EF69EA-865C-4BC3-A502-A0DEBA0CB531 */
	PREDATOR_WMI_IFACE_BATTERY = 2, /* BatteryControl     79772EC5-04B1-4BFD-843C-61E7F77B6CC9 */
	PREDATOR_WMI_IFACE_COUNT
};

#define PREDATOR_WMI_MAX_IN  32
#define PREDATOR_WMI_MAX_OUT 64

/* Values of predator_wmi_call.out_type (ACPI object types) */
#define PREDATOR_WMI_OUT_NONE    0
#define PREDATOR_WMI_OUT_INTEGER 1 /* out[0..7] = little-endian u64 */
#define PREDATOR_WMI_OUT_BUFFER  3

struct predator_wmi_call {
	__u32 iface;      /* in:  enum predator_wmi_iface */
	__u32 method_id;  /* in:  WmiMethodId */
	__u32 in_len;     /* in:  bytes used in @in */
	__u32 out_len;    /* out: bytes written to @out */
	__u32 out_type;   /* out: PREDATOR_WMI_OUT_* */
	__u32 reserved;   /* must be zero */
	__u8  in[PREDATOR_WMI_MAX_IN];
	__u8  out[PREDATOR_WMI_MAX_OUT];
};

struct predator_wmi_info {
	__u32 api_version;
	__u32 iface_mask; /* bit n set = interface n is bound */
};

#define PREDATOR_WMI_IOC_MAGIC 'P'
#define PREDATOR_WMI_IOC_INFO _IOR(PREDATOR_WMI_IOC_MAGIC, 1, struct predator_wmi_info)
#define PREDATOR_WMI_IOC_CALL _IOWR(PREDATOR_WMI_IOC_MAGIC, 2, struct predator_wmi_call)

#endif /* _UAPI_PREDATOR_WMI_H */
