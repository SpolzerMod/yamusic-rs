//! Direct3D compute on the wave worker. Slint still composites the resulting RGBA image.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use super::{frame_layers, spark, Frame, Uniforms, LUT};
use slint::SharedPixelBuffer;
use wgpu::*;

struct NotifyThread(Thread);

impl Wake for NotifyThread {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

// No GUI or audio thread waits here. Adapter and device creation run on the wave worker.
fn wait<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(NotifyThread(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}

pub(super) struct GpuWave {
    device: Device,
    queue: Queue,
    pipeline: ComputePipeline,
    params: Buffer,
    sparks: Buffer,
    failed: Arc<AtomicBool>,
    buffers: Option<Pixels>,
}

struct Pixels {
    output: Buffer,
    readback: Buffer,
    bindings: BindGroup,
}

const PARAM_WORDS: usize = 8 + 3 * 16;

impl GpuWave {
    pub(super) fn new() -> Result<Self, String> {
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::DX12,
            ..InstanceDescriptor::new_without_display_handle()
        });
        let adapter = wait(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::HighPerformance,
            ..Default::default()
        })).map_err(|e| e.to_string())?;
        let info = adapter.get_info();
        if info.device_type == DeviceType::Cpu {
            return Err("Direct3D returned a software adapter".into());
        }
        let (device, queue) = wait(adapter.request_device(&DeviceDescriptor::default()))
            .map_err(|e| e.to_string())?;
        let failed = Arc::new(AtomicBool::new(false));
        let flag = failed.clone();
        device.set_device_lost_callback(move |_, _| flag.store(true, Ordering::Release));
        let flag = failed.clone();
        device.on_uncaptured_error(Arc::new(move |error: Error| {
            flag.store(true, Ordering::Release);
            crate::config::log(format!("vibe GPU: {error}"));
        }));

        let validation = device.push_error_scope(ErrorFilter::Validation);
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Live wave"),
            source: ShaderSource::Wgsl(include_str!("wave.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("Live wave"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(error) = wait(validation.pop()) {
            return Err(error.to_string());
        }
        let params = buffer(&device, "Wave parameters", (PARAM_WORDS * 4) as u64, BufferUsages::UNIFORM | BufferUsages::COPY_DST);
        let sparks = buffer(&device, "Wave sparks", ((LUT + 1) * 4) as u64, BufferUsages::STORAGE | BufferUsages::COPY_DST);
        crate::config::log(format!("vibe GPU: {} ({:?})", info.name, info.backend));
        Ok(Self { device, queue, pipeline, params, sparks, failed, buffers: None })
    }

    pub(super) fn release_buffers(&mut self) {
        self.buffers = None;
    }

    pub(super) fn frame(&mut self, u: &Uniforms, circles: usize, opacity: f32, resolution: usize, crop: [usize; 4]) -> Result<Frame, String> {
        if self.failed.load(Ordering::Acquire) {
            return Err("Direct3D device lost".into());
        }
        let [x, y, right, bottom] = crop;
        let (w, h) = (right.saturating_sub(x), bottom.saturating_sub(y));
        if w == 0 || h == 0 {
            return Ok(Frame { resolution, rect: [0, 0, 1, 1], pixels: SharedPixelBuffer::new(1, 1) });
        }
        let size = (w * h * 4) as u64;
        if self.buffers.as_ref().is_none_or(|p| p.output.size() < size) {
            let output = buffer(&self.device, "Wave pixels", size, BufferUsages::STORAGE | BufferUsages::COPY_SRC);
            let readback = buffer(&self.device, "Wave readback", size, BufferUsages::MAP_READ | BufferUsages::COPY_DST);
            let bindings = self.device.create_bind_group(&BindGroupDescriptor {
                label: Some("Wave buffers"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    BindGroupEntry { binding: 1, resource: self.sparks.as_entire_binding() },
                    BindGroupEntry { binding: 2, resource: output.as_entire_binding() },
                ],
            });
            self.buffers = Some(Pixels { output, readback, bindings });
        }
        let buffers = self.buffers.as_ref().unwrap();
        let mut params = [0u32; PARAM_WORDS];
        params[..4].copy_from_slice(&[x, y, w, h].map(|v| v as u32));
        params[4..8].copy_from_slice(&[resolution as f32, u.time, opacity, circles.min(3) as f32].map(f32::to_bits));
        for (slot, layer) in params[8..].as_chunks_mut::<16>().0.iter_mut().zip(frame_layers(u)) {
            slot.copy_from_slice(&[
                layer.radius + layer.width * 0.5, layer.spark_k, layer.react_k, layer.scale,
                layer.offset, layer.sin_t, layer.strength_base, layer.like,
                layer.c1[0], layer.c1[1], layer.c1[2], layer.shift[0],
                layer.dc[0], layer.dc[1], layer.dc[2], layer.shift[1],
            ].map(f32::to_bits));
        }
        let mut sparks = [0f32; LUT + 1];
        for (i, value) in sparks.iter_mut().take(LUT).enumerate() {
            *value = spark(-std::f32::consts::PI + std::f32::consts::TAU * i as f32 / LUT as f32, u.time);
        }
        sparks[LUT] = sparks[0];
        self.queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&params));
        self.queue.write_buffer(&self.sparks, 0, bytemuck::cast_slice(&sparks));
        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &buffers.bindings, &[]);
            pass.dispatch_workgroups((w as u32).div_ceil(8), (h as u32).div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&buffers.output, 0, &buffers.readback, 0, size);
        let submission = self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        let slice = buffers.readback.slice(..size);
        slice.map_async(MapMode::Read, move |result| { let _ = tx.send(result); });
        self.device.poll(PollType::Wait { submission_index: Some(submission), timeout: Some(Duration::from_secs(2)) })
            .map_err(|e| e.to_string())?;
        rx.try_recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let mut pixels = SharedPixelBuffer::new(w as u32, h as u32);
        {
            let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
            pixels.make_mut_bytes().copy_from_slice(&mapped);
        }
        buffers.readback.unmap();
        Ok(Frame { resolution, rect: [x, y, w, h], pixels })
    }
}

fn buffer(device: &Device, label: &str, size: u64, usage: BufferUsages) -> Buffer {
    device.create_buffer(&BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
}
