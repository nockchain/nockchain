(function () {
  function renderChart(canvas, payload) {
    if (!canvas || !canvas.getContext) {
      return;
    }
    var ctx = canvas.getContext("2d");
    var width = canvas.width;
    var height = canvas.height;
    var padding = 28;
    var usableWidth = width - padding * 2;
    var usableHeight = height - padding * 2;
    var values = payload.values.filter(function (value) { return value !== null && value !== undefined; });
    ctx.clearRect(0, 0, width, height);
    ctx.fillStyle = "#fffdf8";
    ctx.fillRect(0, 0, width, height);
    ctx.strokeStyle = "#d0c4ab";
    ctx.strokeRect(0, 0, width, height);
    ctx.fillStyle = "#241d13";
    ctx.font = "12px sans-serif";
    ctx.fillText(payload.title, padding, 18);
    if (!values.length) {
      ctx.fillText("n/a", padding, height / 2);
      return;
    }
    var max = Math.max.apply(null, values);
    var count = payload.values.length || 1;
    var barWidth = usableWidth / count;
    payload.values.forEach(function (value, index) {
      var x = padding + index * barWidth + 6;
      var y = height - padding;
      if (value === null || value === undefined || max === 0) {
        ctx.fillStyle = "#b9aa92";
        ctx.fillRect(x, y - 2, Math.max(barWidth - 12, 6), 2);
        return;
      }
      var barHeight = (value / max) * usableHeight;
      ctx.fillStyle = "#aa3d26";
      ctx.fillRect(x, y - barHeight, Math.max(barWidth - 12, 6), barHeight);
      ctx.fillStyle = "#241d13";
      ctx.fillText(String(index + 1), x, height - 8);
    });
  }

  function boot() {
    var payloadNode = document.getElementById("chart-payloads");
    if (!payloadNode) {
      return;
    }
    var payloads = JSON.parse(payloadNode.textContent || "[]");
    payloads.forEach(function (payload) {
      renderChart(document.querySelector('[data-chart-id="' + payload.id + '"]'), payload);
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
}());
