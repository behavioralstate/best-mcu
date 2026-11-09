// The camera driver's bindings; QEMU's build leaves the component out, and the module empty.
#if __has_include("esp_camera.h")
#include "esp_camera.h"
#include "img_converters.h"
#endif
