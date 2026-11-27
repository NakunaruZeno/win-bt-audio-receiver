package com.aethersora.companion;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.drawable.Drawable;
import android.provider.Settings;
import android.util.AttributeSet;
import android.view.Choreographer;
import android.view.View;

/**
 * Minimalist Celestial Chrono-Star Audio Orb displaying idle, scanning, and
 * RMS-reactive streaming ring states while honoring system reduced-motion.
 */
public final class CelestialOrbView extends View implements Choreographer.FrameCallback {
    public static final int STATE_IDLE = 0;
    public static final int STATE_SCANNING = 1;
    public static final int STATE_STREAMING = 2;

    private final Paint trackPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint outerRingPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint glowFillPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint starPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF arcBounds = new RectF();

    private Drawable logoDrawable;
    private int state = STATE_IDLE;
    private float targetLevel = 0f;
    private float smoothedLevel = 0f;
    private float scanAngleDeg = -90f;
    private long lastFrameNs = 0L;
    private boolean frameLoopActive = false;
    private boolean initialized = false;

    public CelestialOrbView(Context context) {
        super(context);
        init(context);
    }

    public CelestialOrbView(Context context, AttributeSet attrs) {
        super(context, attrs);
        init(context);
    }

    public CelestialOrbView(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
        init(context);
    }

    private void init(Context context) {
        float density = context.getResources().getDisplayMetrics().density;

        trackPaint.setStyle(Paint.Style.STROKE);
        trackPaint.setStrokeWidth(1.5f * density);
        trackPaint.setColor(0xFF22366E);

        outerRingPaint.setStyle(Paint.Style.STROKE);
        outerRingPaint.setStrokeCap(Paint.Cap.ROUND);
        outerRingPaint.setStrokeWidth(2.4f * density);
        outerRingPaint.setColor(0xFF38BDF8);

        glowFillPaint.setStyle(Paint.Style.FILL);
        glowFillPaint.setColor(0x1E38BDF8);

        starPaint.setStyle(Paint.Style.FILL);

        try {
            logoDrawable = context.getDrawable(R.drawable.ic_chrono_symbol);
        } catch (Throwable ignored) {
            logoDrawable = null;
        }
        initialized = true;
    }

    public void setState(int newState) {
        if (state == newState) {
            return;
        }
        state = newState;
        if (newState != STATE_STREAMING) {
            targetLevel = 0f;
            smoothedLevel = 0f;
        }
        syncFrameLoop();
        invalidate();
    }

    public void setAudioLevel(float level) {
        float clamped = Math.max(0f, Math.min(1f, level));
        targetLevel = clamped;
        if (isReducedMotionEnabled()) {
            smoothedLevel = clamped;
            invalidate();
        }
    }

    private boolean isReducedMotionEnabled() {
        try {
            return Settings.Global.getFloat(
                    getContext().getContentResolver(),
                    Settings.Global.ANIMATOR_DURATION_SCALE,
                    1.0f) == 0f;
        } catch (Throwable ignored) {
            return false;
        }
    }

    private void syncFrameLoop() {
        if (!initialized) {
            return;
        }
        boolean shouldAnimate = isAttachedToWindow()
                && getVisibility() == VISIBLE
                && (state == STATE_SCANNING || state == STATE_STREAMING)
                && !isReducedMotionEnabled();

        if (shouldAnimate && !frameLoopActive) {
            frameLoopActive = true;
            lastFrameNs = 0L;
            Choreographer.getInstance().postFrameCallback(this);
        } else if (!shouldAnimate && frameLoopActive) {
            frameLoopActive = false;
            Choreographer.getInstance().removeFrameCallback(this);
        }
    }

    @Override
    public void doFrame(long frameTimeNanos) {
        if (!frameLoopActive) {
            return;
        }
        if (lastFrameNs == 0L) {
            lastFrameNs = frameTimeNanos;
        }
        float dtSec = Math.min(0.05f, Math.max(0f, (frameTimeNanos - lastFrameNs) / 1_000_000_000f));
        lastFrameNs = frameTimeNanos;

        if (state == STATE_SCANNING) {
            scanAngleDeg = (scanAngleDeg + dtSec * 240f) % 360f;
        } else if (state == STATE_STREAMING) {
            smoothedLevel += (targetLevel - smoothedLevel) * 0.25f;
        }

        invalidate();
        Choreographer.getInstance().postFrameCallback(this);
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        syncFrameLoop();
    }

    @Override
    protected void onDetachedFromWindow() {
        frameLoopActive = false;
        Choreographer.getInstance().removeFrameCallback(this);
        super.onDetachedFromWindow();
    }

    @Override
    protected void onVisibilityChanged(View changedView, int visibility) {
        super.onVisibilityChanged(changedView, visibility);
        if (initialized) {
            syncFrameLoop();
        }
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        if (!initialized) {
            return;
        }
        int w = getWidth();
        int h = getHeight();
        if (w <= 0 || h <= 0) {
            return;
        }

        float cx = w * 0.5f;
        float cy = h * 0.5f;
        float minDim = Math.min(w, h);
        float density = getResources().getDisplayMetrics().density;

        float baseRadius = minDim * 0.34f;
        float levelExpansion = (state == STATE_STREAMING) ? (smoothedLevel * 7f * density) : 0f;

        if (state == STATE_STREAMING) {
            int haloAlpha = Math.round(28 + smoothedLevel * 44);
            glowFillPaint.setColor((haloAlpha << 24) | 0x34D399);
            canvas.drawCircle(cx, cy, baseRadius + levelExpansion + 5f * density, glowFillPaint);
        } else if (state == STATE_SCANNING) {
            glowFillPaint.setColor(0x1E38BDF8);
            canvas.drawCircle(cx, cy, baseRadius + 4f * density, glowFillPaint);
        }

        canvas.drawCircle(cx, cy, baseRadius + levelExpansion, trackPaint);

        if (state == STATE_SCANNING) {
            outerRingPaint.setColor(0xFF38BDF8);
            outerRingPaint.setStrokeWidth(2.4f * density);
            float r = baseRadius + 4f * density;
            arcBounds.set(cx - r, cy - r, cx + r, cy + r);
            if (isReducedMotionEnabled()) {
                canvas.drawCircle(cx, cy, r, outerRingPaint);
            } else {
                canvas.drawArc(arcBounds, scanAngleDeg, 96f, false, outerRingPaint);
            }
        } else if (state == STATE_STREAMING) {
            outerRingPaint.setColor(0xFF34D399);
            outerRingPaint.setStrokeWidth((2.2f + smoothedLevel * 1.4f) * density);
            canvas.drawCircle(cx, cy, baseRadius + levelExpansion, outerRingPaint);
        }

        float dotOffset = baseRadius + 8f * density;
        starPaint.setColor(state == STATE_STREAMING ? 0xFFFACC15 : 0xAA38BDF8);
        canvas.drawCircle(cx - dotOffset * 0.72f, cy - dotOffset * 0.72f, 1.8f * density, starPaint);
        canvas.drawCircle(cx + dotOffset * 0.78f, cy + dotOffset * 0.68f, 1.5f * density, starPaint);
        starPaint.setColor(0x88FACC15);
        canvas.drawCircle(cx + dotOffset * 0.74f, cy - dotOffset * 0.70f, 1.3f * density, starPaint);
        canvas.drawCircle(cx - dotOffset * 0.76f, cy + dotOffset * 0.72f, 1.3f * density, starPaint);

        if (logoDrawable != null) {
            int iconHalf = Math.round(minDim * 0.25f);
            logoDrawable.setBounds(
                    Math.round(cx) - iconHalf,
                    Math.round(cy) - iconHalf,
                    Math.round(cx) + iconHalf,
                    Math.round(cy) + iconHalf);
            logoDrawable.draw(canvas);
        }
    }
}
