//! A dmabuf (a GPU buffer shared between processes and APIs: CEF's frames, GL
//! embeds' too) as a wgpu texture, through Vulkan, without a copy. The texture
//! owns what was made for it (the image, the imported memory): dropping it frees
//! them, and the buffer is the other side's again.

use ash::vk;
use std::os::fd::RawFd;
use wgpu::hal::api::Vulkan;

/// A buffer of one plane (what 8-bit RGBA ones are).
pub struct Dmabuf {
    pub fd: RawFd,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub offset: u32,
    pub modifier: u64,
    pub format: wgpu::TextureFormat,
}

/// The buffer as a texture to sample (its file descriptor is duplicated: the caller keeps its own).
pub fn import(device: &wgpu::Device, buf: &Dmabuf) -> Result<wgpu::Texture, String> {
    let vk_format = match buf.format {
        wgpu::TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        wgpu::TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        f => return Err(format!("{f:?}: not a format salir imports")),
    };
    let size = wgpu::Extent3d { width: buf.width, height: buf.height, depth_or_array_layers: 1 };
    let hal_texture = unsafe {
        let Some(hal) = device.as_hal::<Vulkan>() else { return Err("not Vulkan".into()) };
        let raw = hal.raw_device();
        let instance = hal.shared_instance().raw_instance();
        let fd_device = ash::khr::external_memory_fd::Device::new(instance, raw);

        let layouts = [vk::SubresourceLayout { offset: buf.offset as u64, row_pitch: buf.stride as u64, ..Default::default() }];
        let mut modifier = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default().drm_format_modifier(buf.modifier).plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D { width: buf.width, height: buf.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external)
            .push_next(&mut modifier);
        let image = raw.create_image(&info, None).map_err(|e| format!("vkCreateImage: {e}"))?;

        let fd = libc::dup(buf.fd);
        if fd < 0 {
            raw.destroy_image(image, None);
            return Err("dup".into());
        }
        let mut fd_props = vk::MemoryFdPropertiesKHR::default();
        if let Err(e) = fd_device.get_memory_fd_properties(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT, fd, &mut fd_props) {
            libc::close(fd);
            raw.destroy_image(image, None);
            return Err(format!("vkGetMemoryFdPropertiesKHR: {e}"));
        }
        let needs = raw.get_image_memory_requirements(image);
        let types = needs.memory_type_bits & fd_props.memory_type_bits;
        let Some(type_index) = (0..32).find(|i| types & (1 << i) != 0) else {
            libc::close(fd);
            raw.destroy_image(image, None);
            return Err("no memory type for it".into());
        };
        let mut import = vk::ImportMemoryFdInfoKHR::default().handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT).fd(fd);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let alloc = vk::MemoryAllocateInfo::default().allocation_size(needs.size).memory_type_index(type_index).push_next(&mut import).push_next(&mut dedicated);
        let memory = match raw.allocate_memory(&alloc, None) {
            Ok(m) => m, // (it has the fd now)
            Err(e) => {
                libc::close(fd);
                raw.destroy_image(image, None);
                return Err(format!("vkAllocateMemory: {e}"));
            }
        };
        if let Err(e) = raw.bind_image_memory(image, memory, 0) {
            raw.free_memory(memory, None);
            raw.destroy_image(image, None);
            return Err(format!("vkBindImageMemory: {e}"));
        }
        hal.texture_from_raw(
            image,
            &wgpu::hal::TextureDescriptor {
                label: Some("dmabuf"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: buf.format,
                usage: wgpu::TextureUses::RESOURCE,
                memory_flags: wgpu::hal::MemoryFlags::empty(),
                view_formats: vec![],
            },
            None,                                            // (wgpu destroys the image)
            wgpu::hal::vulkan::TextureMemory::Dedicated(memory), // (and frees the memory)
        )
    };
    Ok(unsafe {
        device.create_texture_from_hal::<Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("dmabuf"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: buf.format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            // (As if ready to sample: it's what the other side drew. Strictly, Vulkan wants the image
            // acquired from a foreign queue; NVIDIA's driver doesn't mind.)
            wgpu::TextureUses::RESOURCE,
        )
    })
}

/// A dmabuf's identity (its inode): the same buffer again, under a new file descriptor.
pub fn identity(fd: RawFd) -> Option<u64> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st.st_ino)
}
