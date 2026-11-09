<script lang="ts">
  import { onMount } from 'svelte';

  let canvas: HTMLCanvasElement | null = null;
  let raf = 0;
  let hover = 0;
  let hoverTarget = 0;
  let speed = 0;
  let rot = 0;
  let time = 0;

  // 对外暴露
  export function setHover(v: boolean) {
    hoverTarget = v ? 1 : 0;
  }

  // 光标视差：px/py ∈ [-0.5, 0.5]，球随光标在球内位置轻微平移
  export function setParallax(px: number, py: number) {
    if (!canvas) return;
    canvas.style.transform = `translate(${(-px * 10).toFixed(2)}px, ${(-py * 10).toFixed(2)}px)`;
  }

  interface Particle {
    r: number;
    kVis: number;
    theta0: number;
    y0: number;
    band: number;
    size: number;
    alpha: number;
    phase: number;
    omega: number;
  }

  const CONFIG = {
    baseSpeed: 0.0035,
    hoverSpeed: 0.012,
    hoverBrightness: 1.0,
    hoverSpeedBoost: 1.0,
  };

  // 外观参数：由父组件通过 props 注入（悬停亮度 / 转速倍率），可持久化
  export let brightness = 1.0;
  export let speedBoost = 1.0;
  $: CONFIG.hoverBrightness = brightness;
  $: CONFIG.hoverSpeedBoost = speedBoost;

  let SIZE = 82; // 跟随容器大小
  let CX = 0, CY = 0, RADIUS = 0;
  const FOCAL = 900;
  let MAX_R = 0;

  const BANDS = 12;
  const particles: Particle[] = [];
  let sprites: HTMLCanvasElement[] = [];

  // 颜色带：暖白 → 蓝 → 紫
  function colorAt(k: number): [number, number, number] {
    const c0: [number, number, number] = [255, 238, 205];
    const c1: [number, number, number] = [150, 190, 255];
    const c2: [number, number, number] = [175, 120, 255];
    let a: [number, number, number], b: [number, number, number], t: number;
    if (k < 0.55) { a = c0; b = c1; t = k / 0.55; }
    else          { a = c1; b = c2; t = (k - 0.55) / 0.45; }
    return [
      Math.round(a[0] + (b[0] - a[0]) * t),
      Math.round(a[1] + (b[1] - a[1]) * t),
      Math.round(a[2] + (b[2] - a[2]) * t),
    ];
  }

  function makeSprite(r: number, g: number, b: number): HTMLCanvasElement {
    const S = 64;
    const c = document.createElement('canvas');
    c.width = c.height = S;
    const x = c.getContext('2d')!;
    const gd = x.createRadialGradient(S / 2, S / 2, 0, S / 2, S / 2, S / 2);
    gd.addColorStop(0,    'rgba(255,255,255,1)');
    gd.addColorStop(0.14, `rgba(${r},${g},${b},.95)`);
    gd.addColorStop(0.40, `rgba(${r},${g},${b},.32)`);
    gd.addColorStop(1,    `rgba(${r},${g},${b},0)`);
    x.fillStyle = gd;
    x.fillRect(0, 0, S, S);
    return c;
  }

  function initSprites() {
    sprites = [];
    for (let i = 0; i < BANDS; i++) {
      const [r, g, b] = colorAt(i / (BANDS - 1));
      sprites.push(makeSprite(r, g, b));
    }
  }

  function pushParticle(r: number, kVis: number, alphaBase: number, sizeBase: number) {
    const theta0 = Math.random() * Math.PI * 2
                 + Math.log(Math.max(r, 5) / 5) * 1.8 * 0.5;
    const thick = 1 + 9 * (1 - kVis) * (1 - kVis);
    const y0 = (Math.random() - 0.5) * thick;
    particles.push({
      r, kVis, theta0, y0,
      band:  Math.min(BANDS - 1, Math.floor(kVis * BANDS)),
      size:  sizeBase * (1.25 - 0.6 * kVis),
      alpha: alphaBase * (1 - 0.55 * kVis),
      phase: Math.random() * Math.PI * 2,
      omega: 1 + (1 - kVis) * 0.30,
    });
  }

  function initParticles() {
    particles.length = 0;
    // 内层密集粒子
    for (let i = 0; i < 1500; i++) {
      const t = Math.pow(Math.random(), 2.4);
      const r = 5 + t * (RADIUS - 5);
      pushParticle(r, r / RADIUS, 0.35 + Math.random() * 0.55, 1.1 + Math.random() * 2.0);
    }
    // 外层晕光
    for (let i = 0; i < 900; i++) {
      const r = (0.55 + Math.random() * 1.55) * RADIUS;
      const kVis = Math.min(1, r / RADIUS);
      pushParticle(r, kVis, 0.14 + Math.random() * 0.30, 0.9 + Math.random() * 1.6);
    }
    // 中心核
    for (let i = 0; i < 300; i++) {
      const t = Math.pow(Math.random(), 1.8);
      const r = 12 + t * (RADIUS - 12);
      pushParticle(r, r / RADIUS, 0.10 + Math.random() * 0.16, 0.8 + Math.random() * 1.2);
    }
  }

  function resize() {
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    const ctx = canvas.getContext('2d', { alpha: false })!;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.scale(dpr, dpr);
    SIZE = rect.width;
    CX = SIZE / 2;
    CY = SIZE / 2;
    RADIUS = SIZE / 2 - 4;
    MAX_R = RADIUS * 2.1;
    initParticles();
  }

  const FADE_IN = 0.82;
  const FADE_OUT = 1.00;
  const seedX = Math.random() * Math.PI * 2;
  const seedY = Math.random() * Math.PI * 2;

  function tick() {
    if (!canvas) return;
    const ctx = canvas.getContext('2d')!;

    time++;
    hover += (hoverTarget - hover) * 0.09;

    const targetSpeed =
      CONFIG.baseSpeed + hover * CONFIG.hoverSpeed * CONFIG.hoverSpeedBoost;
    speed += (targetSpeed - speed) * 0.06;
    rot += speed;

    const brightMul = 1 + hover * 0.75 * CONFIG.hoverBrightness;
    const coreMul   = 1 + hover * 1.00 * CONFIG.hoverBrightness;

    const tt = time * 0.001;
    const tiltX = 0.55 + 0.40 * Math.sin(tt * 0.90 + seedX);
    const tiltY = 0.35 * Math.sin(tt * 0.55 + seedY);
    const cX = Math.cos(tiltX), sX = Math.sin(tiltX);
    const cY = Math.cos(tiltY), sY = Math.sin(tiltY);

    ctx.globalCompositeOperation = 'source-over';
    ctx.fillStyle = 'rgba(0,0,0,0.22)';
    ctx.fillRect(0, 0, SIZE, SIZE);

    ctx.globalCompositeOperation = 'lighter';

    // 中心核光
    const coreR = 34 + hover * 14 * CONFIG.hoverBrightness;
    const gd = ctx.createRadialGradient(CX, CY, 0, CX, CY, Math.max(coreR, 1));
    gd.addColorStop(0,    `rgba(255,250,235,${Math.min(1, 0.07 + (coreMul - 1) * 0.08)})`);
    gd.addColorStop(0.35, `rgba(255,205,150,${Math.min(1, 0.05 + (coreMul - 1) * 0.05)})`);
    gd.addColorStop(0.70, `rgba(150,110,255,${Math.min(1, 0.02 + (coreMul - 1) * 0.02)})`);
    gd.addColorStop(1,    'rgba(60,40,180,0)');
    ctx.fillStyle = gd;
    ctx.beginPath();
    ctx.arc(CX, CY, coreR, 0, Math.PI * 2);
    ctx.fill();

    // 粒子
    for (let i = 0; i < particles.length; i++) {
      const p = particles[i];
      const theta = p.theta0 + rot * p.omega;
      const bx = Math.cos(theta) * p.r;
      const bz = Math.sin(theta) * p.r;
      const by = p.y0;

      const y1 = by * cX - bz * sX;
      const z1 = by * sX + bz * cX;
      const x2 = bx * cY + z1 * sY;
      const z2 = -bx * sY + z1 * cY;

      const persp = FOCAL / (FOCAL - z2);
      const sx = CX + x2 * persp;
      const sy = CY + y1 * persp;

      const dx = sx - CX, dy = sy - CY;
      const distN = Math.sqrt(dx * dx + dy * dy) / RADIUS;
      if (distN > FADE_OUT) continue;

      let edgeFade = 1;
      if (distN > FADE_IN) {
        const e = (distN - FADE_IN) / (FADE_OUT - FADE_IN);
        edgeFade = 1 - e * e * (3 - 2 * e);
      }

      const depth = Math.max(0, Math.min(1, (z2 / MAX_R + 1) * 0.5));
      const twinkle = 0.72 + 0.28 * Math.sin(time * 0.04 + p.phase);
      const a = Math.min(1,
        p.alpha * twinkle *
        (0.60 + 0.40 * depth) *
        brightMul *
        edgeFade
      );
      const grow = 1 + hover * 0.6 * CONFIG.hoverBrightness;
      const s = p.size * 2.0 * grow * persp * (0.70 + 0.50 * depth);

      ctx.globalAlpha = a;
      ctx.drawImage(sprites[p.band], sx - s / 2, sy - s / 2, s, s);
    }
    ctx.globalAlpha = 1;

    raf = requestAnimationFrame(tick);
  }

  onMount(() => {
    initSprites();
    resize();
    window.addEventListener('resize', resize);
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener('resize', resize);
    };
  });
</script>

<canvas
  bind:this={canvas}
  class="galaxy-canvas"
  aria-hidden="true"
></canvas>

<style>
  .galaxy-canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    border-radius: inherit;
    pointer-events: none;
    transition: transform .25s ease;
  }
</style>
