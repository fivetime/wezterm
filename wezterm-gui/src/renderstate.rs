use super::glyphcache::GlyphCache;
use super::quad::*;
use super::utilsprites::{RenderMetrics, UtilSprites};
use crate::termwindow::webgpu::{adapter_info_to_gpu_info, WebGpuState, WebGpuTexture};
use ::window::bitmaps::atlas::OutOfTextureSpace;
use ::window::bitmaps::Texture2d;
use ::window::glium::backend::Context as GliumContext;
use ::window::glium::{
    CapabilitiesSource, IndexBuffer as GliumIndexBuffer, VertexBuffer as GliumVertexBuffer,
};
use ::window::*;
use anyhow::Context;
use std::cell::{Ref, RefCell, RefMut};
use std::convert::TryInto;
use std::rc::Rc;
use wezterm_font::FontConfiguration;
use wgpu::util::DeviceExt;

const INDICES_PER_CELL: usize = 6;

#[derive(Clone)]
pub enum RenderContext {
    Glium(Rc<GliumContext>),
    WebGpu(Rc<WebGpuState>),
}

pub enum RenderFrame<'a> {
    Glium(&'a mut glium::Frame),
    WebGpu,
}

impl RenderContext {
    pub fn allocate_index_buffer(&self, indices: &[u32]) -> anyhow::Result<IndexBuffer> {
        match self {
            Self::Glium(context) => Ok(IndexBuffer::Glium(GliumIndexBuffer::new(
                context,
                glium::index::PrimitiveType::TrianglesList,
                indices,
            )?)),
            Self::WebGpu(state) => Ok(IndexBuffer::WebGpu(WebGpuIndexBuffer::new(indices, state))),
        }
    }

    pub fn allocate_vertex_buffer(&self, num_quads: usize) -> anyhow::Result<VertexBuffer> {
        match self {
            Self::Glium(context) => Ok(VertexBuffer::Glium(
                match GliumVertexBuffer::empty_persistent(context, num_quads * VERTICES_PER_CELL) {
                    Ok(vb) => vb,
                    Err(_) => {
                        GliumVertexBuffer::empty_dynamic(context, num_quads * VERTICES_PER_CELL)?
                    }
                },
            )),
            Self::WebGpu(state) => Ok(VertexBuffer::WebGpu(WebGpuVertexBuffer::new(
                num_quads * VERTICES_PER_CELL,
                state,
            ))),
        }
    }

    pub fn allocate_texture_atlas(&self, size: usize) -> anyhow::Result<Rc<dyn Texture2d>> {
        match self {
            Self::Glium(context) => {
                let caps = context.get_capabilities();
                // You'd hope that allocating a texture would automatically
                // include this check, but it doesn't, and instead, the texture
                // silently fails to bind when attempting to render into it later.
                // So! We check and raise here for ourselves!
                let max_texture_size: usize = caps
                    .max_texture_size
                    .try_into()
                    .context("represent Capabilities.max_texture_size as usize")?;
                if size > max_texture_size {
                    anyhow::bail!(
                        "Cannot use a texture of size {} as it is larger \
                         than the max {} supported by your GPU",
                        size,
                        caps.max_texture_size
                    );
                }
                use crate::glium::texture::SrgbTexture2d;
                let surface: Rc<dyn Texture2d> = Rc::new(SrgbTexture2d::empty_with_format(
                    context,
                    glium::texture::SrgbFormat::U8U8U8U8,
                    glium::texture::MipmapsOption::NoMipmap,
                    size as u32,
                    size as u32,
                )?);
                Ok(surface)
            }
            Self::WebGpu(state) => {
                let texture: Rc<dyn Texture2d> =
                    Rc::new(WebGpuTexture::new(size as u32, size as u32, state)?);
                Ok(texture)
            }
        }
    }

    pub fn renderer_info(&self) -> String {
        match self {
            Self::Glium(ctx) => format!(
                "OpenGL: {} {}",
                ctx.get_opengl_renderer_string(),
                ctx.get_opengl_version_string()
            ),
            Self::WebGpu(state) => {
                let info = adapter_info_to_gpu_info(state.adapter_info.clone());
                format!("WebGPU: {}", info.to_string())
            }
        }
    }
}

pub enum IndexBuffer {
    Glium(GliumIndexBuffer<u32>),
    WebGpu(WebGpuIndexBuffer),
}

impl IndexBuffer {
    pub fn glium(&self) -> &GliumIndexBuffer<u32> {
        match self {
            Self::Glium(g) => g,
            _ => unreachable!(),
        }
    }
    pub fn webgpu(&self) -> &WebGpuIndexBuffer {
        match self {
            Self::WebGpu(g) => g,
            _ => unreachable!(),
        }
    }
}

pub enum VertexBuffer {
    Glium(GliumVertexBuffer<Vertex>),
    WebGpu(WebGpuVertexBuffer),
}

impl VertexBuffer {
    pub fn glium(&self) -> &GliumVertexBuffer<Vertex> {
        match self {
            Self::Glium(g) => g,
            _ => unreachable!(),
        }
    }
    pub fn webgpu(&self) -> &WebGpuVertexBuffer {
        match self {
            Self::WebGpu(g) => g,
            _ => unreachable!(),
        }
    }

    /// Copies `vertices` to the start of the buffer: only the bytes a
    /// frame drew cross to the GPU. OpenGL's buffers are persistently
    /// mapped where the driver can (ARB_buffer_storage), so this is a
    /// memcpy once glium's fence says the GPU is done with the buffer;
    /// elsewhere glBufferSubData. Mapping the whole buffer for reading
    /// and writing every frame, as before, cost ~2.3 ms per layer
    /// (AMD's GL 4.5 on Windows), and glBufferSubData still ~0.2 ms a
    /// call and 2 ms for a pane's 400 KB there. wgpu queues the write.
    fn upload(&self, vertices: &[Vertex]) -> anyhow::Result<()> {
        match self {
            Self::Glium(vb) => {
                vb.slice(0..vertices.len())
                    .ok_or_else(|| anyhow::anyhow!("vertex buffer smaller than its upload"))?
                    .write(vertices);
            }
            Self::WebGpu(vb) => {
                vb.state
                    .queue
                    .write_buffer(&vb.buf, 0, bytemuck::cast_slice(vertices));
            }
        }
        Ok(())
    }
}

pub struct WebGpuVertexBuffer {
    buf: wgpu::Buffer,
    state: Rc<WebGpuState>,
}

impl std::ops::Deref for WebGpuVertexBuffer {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &Self::Target {
        &self.buf
    }
}

impl WebGpuVertexBuffer {
    pub fn new(num_vertices: usize, state: &Rc<WebGpuState>) -> Self {
        Self {
            buf: state.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vertex Buffer"),
                size: (num_vertices * std::mem::size_of::<Vertex>()) as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Rc::clone(state),
        }
    }
}

pub struct WebGpuIndexBuffer {
    buf: wgpu::Buffer,
}

impl std::ops::Deref for WebGpuIndexBuffer {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &Self::Target {
        &self.buf
    }
}

impl WebGpuIndexBuffer {
    pub fn new(indices: &[u32], state: &WebGpuState) -> Self {
        Self {
            buf: state
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Index Buffer"),
                    usage: wgpu::BufferUsages::INDEX,
                    contents: bytemuck::cast_slice(indices),
                }),
        }
    }
}

/// A layer's quads for this frame, gathered on the CPU
pub struct MappedQuads<'a> {
    shadow: RefMut<'a, Vec<Vertex>>,
    next: RefMut<'a, usize>,
}

impl<'a> MappedQuads<'a> {
    /// Makes room for `quads` more quads: running past what the GPU
    /// buffers hold only grows this array, and the buffers grow when it
    /// is uploaded (`TripleVertexBuffer::upload`), rather than the frame
    /// being painted a second time into larger ones
    fn reserve(&mut self, quads: usize) -> std::ops::Range<usize> {
        let start = *self.next * VERTICES_PER_CELL;
        *self.next += quads;
        let end = *self.next * VERTICES_PER_CELL;
        if end > self.shadow.len() {
            let len = end.max(self.shadow.len() + self.shadow.len() / 2);
            self.shadow.resize(len, Vertex::default());
        }
        start..end
    }
}

impl<'a> QuadAllocator for MappedQuads<'a> {
    fn allocate<'b>(&'b mut self) -> anyhow::Result<QuadImpl<'b>> {
        let range = self.reserve(1);
        let mut quad = Quad {
            vert: &mut self.shadow[range],
        };

        quad.set_has_color(false);

        Ok(QuadImpl::Vert(quad))
    }

    fn extend_with(&mut self, vertices: &[Vertex]) {
        let range = self.reserve(vertices.len() / VERTICES_PER_CELL);
        self.shadow[range].copy_from_slice(vertices);
    }
}

/// The GPU side of a `TripleVertexBuffer`: three vertex buffers used in
/// turn, so that the one being filled is not one the GPU may still be
/// drawing from, and the indices of their quads
pub struct GpuQuads {
    bufs: [VertexBuffer; 3],
    pub indices: IndexBuffer,
    capacity: usize,
}

impl GpuQuads {
    fn new(context: &RenderContext, num_quads: usize) -> anyhow::Result<Self> {
        let mut indices = Vec::with_capacity(num_quads * INDICES_PER_CELL);
        for q in 0..num_quads {
            let idx = (q * VERTICES_PER_CELL) as u32;

            // Emit two triangles to form the glyph quad
            indices.push(idx + V_TOP_LEFT as u32);
            indices.push(idx + V_TOP_RIGHT as u32);
            indices.push(idx + V_BOT_LEFT as u32);

            indices.push(idx + V_TOP_RIGHT as u32);
            indices.push(idx + V_BOT_LEFT as u32);
            indices.push(idx + V_BOT_RIGHT as u32);
        }
        log::trace!(
            "GpuQuads num_quads={}, {} bytes per buffer",
            num_quads,
            num_quads * VERTICES_PER_CELL * std::mem::size_of::<Vertex>()
        );
        Ok(Self {
            bufs: [
                context.allocate_vertex_buffer(num_quads)?,
                context.allocate_vertex_buffer(num_quads)?,
                context.allocate_vertex_buffer(num_quads)?,
            ],
            indices: context.allocate_index_buffer(&indices)?,
            capacity: num_quads,
        })
    }

    pub fn vertices(&self, index: usize) -> &VertexBuffer {
        &self.bufs[index]
    }
}

pub struct TripleVertexBuffer {
    pub index: RefCell<usize>,
    gpu: RefCell<GpuQuads>,
    /// This frame's quads, gathered before they are uploaded
    shadow: RefCell<Vec<Vertex>>,
    pub next_quad: RefCell<usize>,
    context: RenderContext,
}

/// A trait to avoid broadly-scoped transmutes; we only want to
/// transmute to extend a lifetime to static, and not to change
/// the underlying type.
/// These ExtendStatic trait impls constrain the transmutes in that way,
/// so that the type checker can still catch issues.
unsafe trait ExtendStatic {
    type T;
    unsafe fn extend_lifetime(self) -> Self::T;
}

unsafe impl<'a, T: 'static> ExtendStatic for Ref<'a, T> {
    type T = Ref<'static, T>;
    unsafe fn extend_lifetime(self) -> Self::T {
        std::mem::transmute(self)
    }
}

unsafe impl<'a> ExtendStatic for MappedQuads<'a> {
    type T = MappedQuads<'static>;
    unsafe fn extend_lifetime(self) -> Self::T {
        std::mem::transmute(self)
    }
}

impl TripleVertexBuffer {
    fn new(context: &RenderContext, num_quads: usize) -> anyhow::Result<Self> {
        Ok(Self {
            index: RefCell::new(0),
            gpu: RefCell::new(GpuQuads::new(context, num_quads)?),
            shadow: RefCell::new(vec![Vertex::default(); num_quads * VERTICES_PER_CELL]),
            next_quad: RefCell::new(0),
            context: context.clone(),
        })
    }

    pub fn clear_quad_allocation(&self) {
        *self.next_quad.borrow_mut() = 0;
    }

    pub fn vertex_index_count(&self) -> (usize, usize) {
        let num_quads = *self.next_quad.borrow();
        (num_quads * VERTICES_PER_CELL, num_quads * INDICES_PER_CELL)
    }

    pub fn map(&self) -> MappedQuads<'_> {
        MappedQuads {
            shadow: self.shadow.borrow_mut(),
            next: self.next_quad.borrow_mut(),
        }
    }

    /// Hands this frame's quads to the vertex buffer whose turn it is,
    /// growing the buffers first (by half again, in steps of 128 quads)
    /// when the frame drew more than they hold
    pub fn upload(&self) -> anyhow::Result<()> {
        let quads = *self.next_quad.borrow();
        if quads == 0 {
            return Ok(());
        }
        let capacity = self.gpu.borrow().capacity;
        if quads > capacity {
            let num_quads = (quads.max(capacity + capacity / 2) + 127) & !127;
            *self.gpu.borrow_mut() =
                GpuQuads::new(&self.context, num_quads).with_context(|| {
                    format!("Failed to allocate {num_quads} quads (needed {quads})")
                })?;
        }
        let _t = crate::stats::Timed::new("quad.upload");
        let gpu = self.gpu.borrow();
        let shadow = self.shadow.borrow();
        gpu.bufs[*self.index.borrow()].upload(&shadow[..quads * VERTICES_PER_CELL])
    }

    /// The GPU buffers, and which vertex buffer's turn it is
    pub fn gpu(&self) -> (Ref<'_, GpuQuads>, usize) {
        (self.gpu.borrow(), *self.index.borrow())
    }

    pub fn next_index(&self) {
        let mut index = self.index.borrow_mut();
        *index += 1;
        if *index >= 3 {
            *index = 0;
        }
    }
}

pub struct RenderLayer {
    pub vb: RefCell<[TripleVertexBuffer; 3]>,
    zindex: i8,
}

/// The layer drawn last and without blending: what it draws replaces the
/// pixels beneath, colour and alpha, so a transparent quad there clears
/// them (the content's top corners, for a window that draws its own
/// round ones beneath; see `WindowState::CLIENT_EDGE`).
pub const ERASE_ZINDEX: i8 = i8::MAX;

/// The layer drawn after every ordinary one, multiplying the pixels
/// beneath, colour and alpha, by its quads' alpha: a mask. The content's
/// top corners are cut to a window's own round ones this way, so that
/// what the content drew inside the arc stays (a button by the corner)
/// and only what lies outside it is cleared (`TermWindow::mask_edge_corners`).
pub const MASK_ZINDEX: i8 = i8::MAX - 1;

impl RenderLayer {
    pub fn zindex(&self) -> i8 {
        self.zindex
    }

    pub fn new(context: &RenderContext, num_quads: usize, zindex: i8) -> anyhow::Result<Self> {
        let vb = [
            TripleVertexBuffer::new(context, 32)?,
            TripleVertexBuffer::new(context, num_quads)?,
            TripleVertexBuffer::new(context, 32)?,
        ];

        Ok(Self {
            vb: RefCell::new(vb),
            zindex,
        })
    }

    pub fn clear_quad_allocation(&self) {
        for vb in self.vb.borrow().iter() {
            vb.clear_quad_allocation();
        }
    }

    pub fn quad_allocator(&self) -> TripleLayerQuadAllocator<'_> {
        // We're creating a self-referential struct here to manage the lifetimes
        // of these related items.  The transmutes are safe because we're only
        // transmuting the lifetimes (not the types), and we're keeping hold
        // of the owner in the returned struct.
        unsafe {
            let vbs = self.vb.borrow().extend_lifetime();
            let layer0 = vbs[0].map().extend_lifetime();
            let layer1 = vbs[1].map().extend_lifetime();
            let layer2 = vbs[2].map().extend_lifetime();
            TripleLayerQuadAllocator::Gpu(BorrowedLayers {
                layers: [layer0, layer1, layer2],
                _owner: vbs,
            })
        }
    }
}

pub struct BorrowedLayers {
    pub layers: [MappedQuads<'static>; 3],

    // layers references _owner, so it must be dropped after layers.
    _owner: Ref<'static, [TripleVertexBuffer; 3]>,
}

impl TripleLayerQuadAllocatorTrait for BorrowedLayers {
    fn allocate(&mut self, layer_num: usize) -> anyhow::Result<QuadImpl<'_>> {
        self.layers[layer_num].allocate()
    }

    fn extend_with(&mut self, layer_num: usize, vertices: &[Vertex]) {
        self.layers[layer_num].extend_with(vertices)
    }
}

pub struct RenderState {
    pub context: RenderContext,
    pub glyph_cache: RefCell<GlyphCache>,
    pub util_sprites: UtilSprites,
    pub glyph_prog: Option<glium::Program>,
    pub layers: RefCell<Vec<Rc<RenderLayer>>>,
}

impl RenderState {
    pub fn new(
        context: RenderContext,
        fonts: &Rc<FontConfiguration>,
        metrics: &RenderMetrics,
        mut atlas_size: usize,
    ) -> anyhow::Result<Self> {
        loop {
            let glyph_cache = RefCell::new(GlyphCache::new_gl(&context, fonts, atlas_size)?);
            let result = UtilSprites::new(&mut *glyph_cache.borrow_mut(), metrics);
            match result {
                Ok(util_sprites) => {
                    let glyph_prog = match &context {
                        RenderContext::Glium(context) => {
                            Some(Self::compile_prog(&context, Self::glyph_shader)?)
                        }
                        RenderContext::WebGpu(_) => None,
                    };

                    let main_layer = Rc::new(RenderLayer::new(&context, 1024, 0)?);

                    return Ok(Self {
                        context,
                        glyph_cache,
                        util_sprites,
                        glyph_prog,
                        layers: RefCell::new(vec![main_layer]),
                    });
                }
                Err(OutOfTextureSpace {
                    size: Some(size), ..
                }) => {
                    atlas_size = size;
                }
                Err(OutOfTextureSpace { size: None, .. }) => {
                    anyhow::bail!("requested texture size is impossible!?")
                }
            };
        }
    }

    pub fn layer_for_zindex(&self, zindex: i8) -> anyhow::Result<Rc<RenderLayer>> {
        if let Some(layer) = self
            .layers
            .borrow()
            .iter()
            .find(|l| l.zindex == zindex)
            .map(Rc::clone)
        {
            return Ok(layer);
        }

        let layer = Rc::new(RenderLayer::new(&self.context, 128, zindex)?);
        let mut layers = self.layers.borrow_mut();
        layers.push(Rc::clone(&layer));

        // Keep the layers sorted by zindex so that they are rendered in
        // the correct order when the layers array is iterated.
        layers.sort_by(|a, b| a.zindex.cmp(&b.zindex));

        Ok(layer)
    }

    fn compile_prog(
        context: &Rc<GliumContext>,
        fragment_shader: fn(&str) -> (String, String),
    ) -> anyhow::Result<glium::Program> {
        let mut errors = vec![];

        let caps = context.get_capabilities();
        log::trace!("Compiling shader. context.capabilities.srgb={}", caps.srgb);

        for version in &["330 core", "330", "320 es", "300 es"] {
            let (vertex_shader, fragment_shader) = fragment_shader(version);
            let source = glium::program::ProgramCreationInput::SourceCode {
                vertex_shader: &vertex_shader,
                fragment_shader: &fragment_shader,
                outputs_srgb: true,
                tessellation_control_shader: None,
                tessellation_evaluation_shader: None,
                transform_feedback_varyings: None,
                uses_point_size: false,
                geometry_shader: None,
            };
            match glium::Program::new(context, source) {
                Ok(prog) => {
                    return Ok(prog);
                }
                Err(err) => errors.push(format!("shader version: {}: {:#}", version, err)),
            };
        }

        anyhow::bail!("Failed to compile shaders: {}", errors.join("\n"))
    }

    fn glyph_shader(version: &str) -> (String, String) {
        (
            format!(
                "#version {}\n{}",
                version,
                include_str!("glyph-vertex.glsl")
            ),
            format!("#version {}\n{}", version, include_str!("glyph-frag.glsl")),
        )
    }

    pub fn config_changed(&mut self) {
        self.glyph_cache.borrow_mut().config_changed();
    }

    pub fn recreate_texture_atlas(
        &mut self,
        fonts: &Rc<FontConfiguration>,
        metrics: &RenderMetrics,
        size: Option<usize>,
    ) -> anyhow::Result<()> {
        // We make a a couple of passes at resizing; if the user has selected a large
        // font size (or a large scaling factor) then the `size==None` case will not
        // be able to fit the initial utility glyphs and apply_scale_change won't
        // be able to deal with that error situation.  Rather than make every
        // caller know how to deal with OutOfTextureSpace we try to absorb
        // and accomodate that here.
        let mut size = size;
        let mut attempt = 10;
        loop {
            match self.recreate_texture_atlas_impl(fonts, metrics, size) {
                Ok(_) => return Ok(()),
                Err(err) => {
                    attempt -= 1;
                    if attempt == 0 {
                        return Err(err);
                    }

                    if let Some(&OutOfTextureSpace {
                        size: Some(needed_size),
                        ..
                    }) = err.downcast_ref::<OutOfTextureSpace>()
                    {
                        size.replace(needed_size);
                        continue;
                    }

                    return Err(err);
                }
            }
        }
    }

    fn recreate_texture_atlas_impl(
        &mut self,
        fonts: &Rc<FontConfiguration>,
        metrics: &RenderMetrics,
        size: Option<usize>,
    ) -> anyhow::Result<()> {
        let size = size.unwrap_or_else(|| self.glyph_cache.borrow().atlas.size());
        let mut new_glyph_cache = GlyphCache::new_gl(&self.context, fonts, size)?;
        self.util_sprites = UtilSprites::new(&mut new_glyph_cache, metrics)?;

        let mut glyph_cache = self.glyph_cache.borrow_mut();

        // Steal the decoded image cache; without this, any animating gifs
        // would reset back to frame 0 each time we filled the texture
        std::mem::swap(
            &mut glyph_cache.image_cache,
            &mut new_glyph_cache.image_cache,
        );

        *glyph_cache = new_glyph_cache;
        Ok(())
    }
}
