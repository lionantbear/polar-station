// Native GPU presentation adapted from iTree-View 8f4bf46.
// Included after GtkSurfaceDisplay; no second scene, input or event owner.
#pragma once
#include "servers/rendering/rendering_device.h"
#include "servers/rendering/rendering_server.h"
#include "servers/rendering/renderer_rd/renderer_compositor_rd.h"
#ifdef VULKAN_ENABLED
// Volk's optional Xlib typedef otherwise collides with Godot's scene Window.
// Only rename that C handle while importing the platform Vulkan declarations.
#define Window XlibNativeWindow
#include "drivers/vulkan/godot_vulkan.h"
#undef Window
#include "drivers/vulkan/rendering_context_driver_vulkan.h"
#include "drivers/vulkan/vulkan_hooks.h"
#include <vector>
static unsigned char gtk_device_uuid[VK_UUID_SIZE];
class NativeVulkanHooks : public VulkanHooks {
    VkInstance vk_instance=VK_NULL_HANDLE;
    VkPhysicalDevice physical=VK_NULL_HANDLE;
public:
    bool create_vulkan_instance(const VkInstanceCreateInfo *info,VkInstance *out) override {
        VkResult result=vkCreateInstance(info,nullptr,out);vk_instance=*out;return result==VK_SUCCESS;
    }
    bool get_physical_device(VkPhysicalDevice *out) override {
        uint32_t count=0;vkEnumeratePhysicalDevices(vk_instance,&count,nullptr);
        std::vector<VkPhysicalDevice> devices(count);vkEnumeratePhysicalDevices(vk_instance,&count,devices.data());
        for(auto candidate:devices){
            VkPhysicalDeviceIDProperties ids{};ids.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES;
            VkPhysicalDeviceProperties2 props{};props.sType=VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2;props.pNext=&ids;
            vkGetPhysicalDeviceProperties2(candidate,&props);
            if(memcmp(ids.deviceUUID,gtk_device_uuid,VK_UUID_SIZE)==0){physical=candidate;*out=candidate;return true;}
        }
        fprintf(stderr,"RD_GPU_MISMATCH: no Vulkan GPU matches the GTK device UUID\n");return false;
    }
    bool create_vulkan_device(const VkDeviceCreateInfo *input,VkDevice *out) override {
        std::vector<const char*> names(input->ppEnabledExtensionNames,input->ppEnabledExtensionNames+input->enabledExtensionCount);
        names.push_back(VK_KHR_EXTERNAL_SEMAPHORE_FD_EXTENSION_NAME);
        VkDeviceCreateInfo info=*input;info.enabledExtensionCount=names.size();info.ppEnabledExtensionNames=names.data();
        return vkCreateDevice(physical,&info,nullptr,out)==VK_SUCCESS;
    }
    void set_direct_queue_family_and_index(uint32_t,uint32_t) override {}
    bool use_fragment_density_offsets() override {return false;}
    void get_fragment_density_offsets(LocalVector<VkOffset2D> &,const Vector2i &) override {}
    bool use_subsampled_images() override {return false;}
};
static NativeVulkanHooks *vulkan_hooks;
extern "C" LIBGODOT_API void polar_godot_device_uuid(const unsigned char *uuid){memcpy(gtk_device_uuid,uuid,VK_UUID_SIZE);}
#endif
#ifdef METAL_ENABLED
#include "drivers/metal/rendering_context_driver_metal.h"
#endif

// A native Pane has no Godot screen/swapchain. Only suppress the engine's
// screen boot splash; every actual canvas renderer remains upstream RD.
class NativeRDCompositor : public RendererCompositorRD {
public:
    void set_boot_image_with_stretch(const Ref<Image> &,const Color &,RSE::SplashStretchMode,bool) override {}
    static RendererCompositor *create_native(){return memnew(NativeRDCompositor);}
    static void make_native_current(){_create_func=create_native;}
};
static RenderingDevice *rd;
class NativeRDDisplay : public GtkSurfaceDisplay {
    RenderingContextDriver *context = nullptr;
public:
    NativeRDDisplay(Size2i p_size) : GtkSurfaceDisplay(p_size) {}
    String get_name() const override { return "native_rd"; }
    static Vector<String> drivers() {
#ifdef METAL_ENABLED
        return {"metal"};
#else
        return {"vulkan"};
#endif
    }
    static DisplayServer *create(const String &, DisplayServerEnums::WindowMode, DisplayServerEnums::VSyncMode, uint32_t, const Vector2i *, const Vector2i &size, int, DisplayServerEnums::Context, int64_t, Error &error) {
        auto *display = memnew(NativeRDDisplay(size));
#ifdef METAL_ENABLED
        display->context = memnew(RenderingContextDriverMetal);
#else
        display->context = memnew(RenderingContextDriverVulkan);
#endif
        error = display->context->initialize();
        if (error != OK) { memdelete(display); return nullptr; }
        rd = memnew(RenderingDevice);
        error = rd->initialize(display->context);
        if (error != OK) { memdelete(display); return nullptr; }
        NativeRDCompositor::make_native_current();
        return display;
    }
    ~NativeRDDisplay() override {
        if (rd) { memdelete(rd); rd = nullptr; }
        if (context) memdelete(context);
    }
};

static uint64_t rd_resource(int kind) {
    return rd ? rd->get_driver_resource(static_cast<RenderingDevice::DriverResource>(kind),RID(),0) : 0;
}
static void register_native_rd() {
#ifdef VULKAN_ENABLED
    vulkan_hooks = new NativeVulkanHooks;
#endif
    DisplayServer::register_create_function("native_rd",NativeRDDisplay::create,NativeRDDisplay::drivers);
}
static void stop_native_rd() {
#ifdef VULKAN_ENABLED
    delete vulkan_hooks; vulkan_hooks=nullptr;
#endif
}
#ifdef VULKAN_ENABLED
struct NativeExport {
VkDevice device{};
VkQueue queue{};
uint32_t family{};
VkImage exported_image{};
VkDeviceMemory exported_memory{};
VkDeviceSize allocation_size{};
RID export_rid;
VkCommandPool pool{};
VkCommandBuffer acquire_cmd{},release_cmd{};
VkSemaphore ready_sem{},done_sem{};
bool shared_with_gl=false;
int export_width=0,export_height=0;
bool vk_ok(VkResult result, const char *operation) {
    if (result == VK_SUCCESS) return true;
    fprintf(stderr,"RD_VULKAN_ERROR %s result=%d\n",operation,int(result));return false;
}
// Reusable command buffers plus binary external semaphores transfer ownership.
// The GPU, not a CPU wait, orders each producer/consumer round trip.
bool ownership(bool to_gl) {
    VkCommandBuffer &cmd=to_gl?release_cmd:acquire_cmd;
    if(!cmd){
        VkCommandBufferAllocateInfo ai{};ai.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;ai.commandPool=pool;ai.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY;ai.commandBufferCount=1;
        if(!vk_ok(vkAllocateCommandBuffers(device,&ai,&cmd),"allocate command"))return false;
        VkCommandBufferBeginInfo bi{};bi.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;bi.flags=VK_COMMAND_BUFFER_USAGE_SIMULTANEOUS_USE_BIT;
        vkBeginCommandBuffer(cmd,&bi);
        VkImageMemoryBarrier barrier{};barrier.sType=VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;barrier.image=exported_image;
        barrier.subresourceRange={VK_IMAGE_ASPECT_COLOR_BIT,0,1,0,1};
        barrier.oldLayout=to_gl?VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL:VK_IMAGE_LAYOUT_GENERAL;
        barrier.newLayout=to_gl?VK_IMAGE_LAYOUT_GENERAL:VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
        barrier.srcQueueFamilyIndex=to_gl?family:VK_QUEUE_FAMILY_EXTERNAL;
        barrier.dstQueueFamilyIndex=to_gl?VK_QUEUE_FAMILY_EXTERNAL:family;
        barrier.srcAccessMask=to_gl?VK_ACCESS_TRANSFER_WRITE_BIT:0;
        barrier.dstAccessMask=to_gl?0:VK_ACCESS_TRANSFER_WRITE_BIT;
        vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,0,0,nullptr,0,nullptr,1,&barrier);
        vkEndCommandBuffer(cmd);
    }
    VkPipelineStageFlags stage=VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
    VkSubmitInfo si{};si.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO;si.commandBufferCount=1;si.pCommandBuffers=&cmd;
    if(to_gl){si.signalSemaphoreCount=1;si.pSignalSemaphores=&ready_sem;}
    else{si.waitSemaphoreCount=1;si.pWaitSemaphores=&done_sem;si.pWaitDstStageMask=&stage;}
    return vk_ok(vkQueueSubmit(queue,1,&si,VK_NULL_HANDLE),"ownership submit");
}
int semaphore_fd(int ready) {
    VkSemaphore &sem=ready?ready_sem:done_sem;
    VkExportSemaphoreCreateInfo ex{};ex.sType=VK_STRUCTURE_TYPE_EXPORT_SEMAPHORE_CREATE_INFO;ex.handleTypes=VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_FD_BIT;
    VkSemaphoreCreateInfo ci{};ci.sType=VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO;ci.pNext=&ex;
    if(!vk_ok(vkCreateSemaphore(device,&ci,nullptr,&sem),"semaphore"))return -1;
    auto get_fd=(PFN_vkGetSemaphoreFdKHR)vkGetDeviceProcAddr(device,"vkGetSemaphoreFdKHR");
    if(!get_fd)return -1;
    VkSemaphoreGetFdInfoKHR fi{};fi.sType=VK_STRUCTURE_TYPE_SEMAPHORE_GET_FD_INFO_KHR;fi.semaphore=sem;fi.handleType=VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_FD_BIT;
    int fd=-1;return vk_ok(get_fd(device,&fi,&fd),"semaphore fd")?fd:-1;
}
void destroy() {
    if (!device) return;
    vkDeviceWaitIdle(device);
    if (export_rid.is_valid()) {rd->free_rid(export_rid);export_rid=RID();for(uint32_t i=0;i<=rd->get_frame_delay();i++)rd->swap_buffers(false);vkDeviceWaitIdle(device);}
    if(acquire_cmd)vkFreeCommandBuffers(device,pool,1,&acquire_cmd);
    if(release_cmd)vkFreeCommandBuffers(device,pool,1,&release_cmd);
    acquire_cmd=release_cmd=VK_NULL_HANDLE;
    if(ready_sem)vkDestroySemaphore(device,ready_sem,nullptr);
    if(done_sem)vkDestroySemaphore(device,done_sem,nullptr);
    ready_sem=done_sem=VK_NULL_HANDLE;
    if (exported_image) vkDestroyImage(device,exported_image,nullptr);
    if (exported_memory) vkFreeMemory(device,exported_memory,nullptr);
    exported_image=VK_NULL_HANDLE;exported_memory=VK_NULL_HANDLE;shared_with_gl=false;
}
int create(RID source,int width,int height,uint64_t *size) {
    if (exported_image) return -1; // Host must release its GL reference first.
    device=(VkDevice)rd_resource(RenderingDevice::DRIVER_RESOURCE_LOGICAL_DEVICE);
    auto physical=(VkPhysicalDevice)rd_resource(RenderingDevice::DRIVER_RESOURCE_PHYSICAL_DEVICE);
    queue=(VkQueue)rd_resource(RenderingDevice::DRIVER_RESOURCE_COMMAND_QUEUE);
    family=(uint32_t)rd_resource(RenderingDevice::DRIVER_RESOURCE_QUEUE_FAMILY);
    if (!pool) {VkCommandPoolCreateInfo pi{};pi.sType=VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;pi.queueFamilyIndex=family;if(!vk_ok(vkCreateCommandPool(device,&pi,nullptr,&pool),"pool"))return -1;}
    if (rd->texture_get_format(source).format!=RenderingDevice::DATA_FORMAT_R8G8B8A8_UNORM) {fprintf(stderr,"RD_EXPORT unsupported root format\n");return -1;}
    VkExternalMemoryImageCreateInfo external{};external.sType=VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO;external.handleTypes=VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT;
    VkImageCreateInfo ci{};ci.sType=VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;ci.pNext=&external;ci.imageType=VK_IMAGE_TYPE_2D;ci.format=VK_FORMAT_R8G8B8A8_UNORM;ci.extent={(uint32_t)width,(uint32_t)height,1};ci.mipLevels=1;ci.arrayLayers=1;ci.samples=VK_SAMPLE_COUNT_1_BIT;ci.tiling=VK_IMAGE_TILING_OPTIMAL;ci.usage=VK_IMAGE_USAGE_TRANSFER_DST_BIT|VK_IMAGE_USAGE_SAMPLED_BIT|VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;ci.sharingMode=VK_SHARING_MODE_EXCLUSIVE;
    if(!vk_ok(vkCreateImage(device,&ci,nullptr,&exported_image),"image"))return -1;
    VkMemoryRequirements req;vkGetImageMemoryRequirements(device,exported_image,&req);
    VkPhysicalDeviceMemoryProperties props;vkGetPhysicalDeviceMemoryProperties(physical,&props);
    uint32_t memory_type=UINT32_MAX;
    for(uint32_t i=0;i<props.memoryTypeCount;i++)if((req.memoryTypeBits&(1u<<i))&&(props.memoryTypes[i].propertyFlags&VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)){memory_type=i;break;}
    if(memory_type==UINT32_MAX)return -1;
    VkMemoryDedicatedAllocateInfo dedicated{};dedicated.sType=VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO;dedicated.image=exported_image;
    VkExportMemoryAllocateInfo export_info{};export_info.sType=VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO;export_info.pNext=&dedicated;export_info.handleTypes=VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT;
    VkMemoryAllocateInfo ma{};ma.sType=VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;ma.pNext=&export_info;ma.allocationSize=req.size;ma.memoryTypeIndex=memory_type;
    if(!vk_ok(vkAllocateMemory(device,&ma,nullptr,&exported_memory),"memory"))return -1;
    if(!vk_ok(vkBindImageMemory(device,exported_image,exported_memory,0),"bind"))return -1;
    export_rid=rd->texture_create_from_extension(RenderingDevice::TEXTURE_TYPE_2D,RenderingDevice::DATA_FORMAT_R8G8B8A8_UNORM,RenderingDevice::TEXTURE_SAMPLES_1,RenderingDevice::TEXTURE_USAGE_CAN_COPY_TO_BIT|RenderingDevice::TEXTURE_USAGE_SAMPLING_BIT,(uint64_t)exported_image,width,height,1,1);
    if(!export_rid.is_valid())return -1;
    auto get_fd=(PFN_vkGetMemoryFdKHR)vkGetDeviceProcAddr(device,"vkGetMemoryFdKHR");
    if(!get_fd)return -1;
    VkMemoryGetFdInfoKHR fi{};fi.sType=VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR;fi.memory=exported_memory;fi.handleType=VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT;
    int fd=-1;if(!vk_ok(get_fd(device,&fi,&fd),"export fd"))return -1;
    export_width=width;export_height=height;allocation_size=req.size;*size=allocation_size;return fd;
}
int copy(RID source) {
    if(!export_rid.is_valid())return 0;
    if(shared_with_gl && !ownership(false))return 0;
    if(rd->texture_copy(source,export_rid,Vector3(),Vector3(),Vector3(export_width,export_height,1),0,0,0,0)!=OK)return 0;
    rd->swap_buffers(false); // Existing main-device flush; submit() is local-only.
    if(!ownership(true))return 0;
    shared_with_gl=true;return 1;
}
~NativeExport() { destroy(); if(pool) vkDestroyCommandPool(device,pool,nullptr); }
};
#endif
