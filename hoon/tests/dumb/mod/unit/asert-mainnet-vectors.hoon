::  tests/dumb/mod/unit/asert-mainnet-vectors.hoon
::
::    Phase-2 cross-check: verify that ASERT targets computed using the
::    cutover constants (the hardcoded anchor-min-timestamp.zk-asert,
::    anchor-target-atom.zk-asert, and anchor-height.zk-asert baked into
::    blockchain-constants:v1) match the targets observed on canonical
::    mainnet for a range of post-activation blocks.
::
::    This is the load-bearing consensus-continuity property: phase-2
::    nodes derive anchor-min-ts by reading a constant instead of walking
::    .blocks / .min-timestamps. If our constant matches the median-of-11
::    the phase-1 walk would have returned at the anchor, then every
::    post-activation target computation produces the same value on
::    phase-2 nodes as it did on phase-1 nodes — by construction. This
::    test pins that property against observed mainnet outputs from the
::    public gRPC node, so any future drift in the polynomial, the
::    constants, or the wrapper plumbing fails CI.
::
::    Vectors are produced by `scripts/aletheia_asert_vectors.rs` against
::    a Nockchain public gRPC node; for each post-activation height N the
::    `parent-min-ts` field is `median-of-11(timestamps[N-11..=N-1])`,
::    matching what `min-timestamps[parent.digest]` holds on a node that
::    just accepted block (N-1). The observed-target is the chain-
::    reported `target.display` for block N (decimal atom).
::
/=  asert  /apps/dumbnet/lib/asert
/=  txe    /common/tx-engine
/=  *      /common/zeke
/=  *      /common/test
::
=>
|%
::  Pins on the phase-2 cutover values. These match the realnet bunt of
::  blockchain-constants:v1 (open/hoon/common/tx-engine-1.hoon). If you
::  change the field defaults there without updating these, the test
::  catches it.
++  anchor-min-timestamp  9.223.372.093.639.027.842
++  anchor-target-atom    ^~((bex 291))
++  anchor-height         65.499
++  ideal-block-time      150
++  half-life             ^~((mul 12 ^~((mul 60 60))))
++  max-target-atom       max-tip5-atom:tip5
::
::  Mainnet ASERT cross-check vectors. Each entry is
::  [height parent-min-ts observed-target], where parent-min-ts is the
::  median-of-11 of timestamps for blocks (height-11)..=(height-1) and
::  observed-target is the chain-reported target.display for block
::  height as a decimal atom.
::
::  TODO: fill in observed mainnet vectors from gRPC. Run
::  `scripts/aletheia_asert_vectors.rs` against a reachable public
::  Nockchain gRPC node and paste the emitted `:~ ... ==` list here.
::  Range fetched by the script: 65.500..=65.520 (21 blocks).
++  asert-vectors
  ^-  (list [height=@ parent-min-ts=@ observed-target=@])
  :~
    [height=65.500 parent-min-ts=9.223.372.093.639.027.842 observed-target=3.978.585.891.278.293.137.243.057.985.174.566.720.803.649.206.378.781.739.523.711.815.145.275.976.100.267.004.264.448]
    [height=65.501 parent-min-ts=9.223.372.093.639.027.872 observed-target=3.970.966.986.716.595.356.043.045.720.508.743.232.591.661.017.056.874.129.838.802.363.350.831.210.856.727.827.185.664]
    [height=65.502 parent-min-ts=9.223.372.093.639.028.257 observed-target=3.985.931.608.027.021.675.292.472.200.509.743.151.669.390.807.238.947.642.088.445.230.819.601.207.928.699.198.898.176]
    [height=65.503 parent-min-ts=9.223.372.093.639.028.875 observed-target=4.016.103.684.259.402.529.925.190.093.250.095.929.688.180.688.457.976.184.027.887.442.308.358.565.108.292.593.385.472]
    [height=65.504 parent-min-ts=9.223.372.093.639.028.925 observed-target=4.009.607.885.151.022.748.344.303.142.499.154.788.344.095.140.589.895.592.503.701.694.563.294.103.904.637.677.469.696]
    [height=65.505 parent-min-ts=9.223.372.093.639.028.951 observed-target=4.001.594.375.970.591.615.926.760.362.133.507.772.854.195.212.378.805.516.978.537.968.373.121.123.728.166.192.414.720]
    [height=65.506 parent-min-ts=9.223.372.093.639.029.061 observed-target=3.999.044.623.049.545.346.521.178.568.380.801.904.289.227.053.402.549.583.856.894.964.585.338.811.853.834.356.260.864]
    [height=65.507 parent-min-ts=9.223.372.093.639.029.317 observed-target=4.005.843.964.172.335.398.269.396.685.054.684.220.462.475.477.339.232.072.181.276.308.019.424.976.852.052.586.004.480]
    [height=65.508 parent-min-ts=9.223.372.093.639.029.337 observed-target=3.997.526.912.977.493.995.684.522.738.766.096.030.143.412.673.059.540.099.855.916.986.140.230.292.881.017.787.121.664]
    [height=65.509 parent-min-ts=9.223.372.093.639.029.395 observed-target=3.991.577.489.495.052.700.404.831.886.676.449.003.491.820.302.114.942.922.572.083.310.635.404.898.507.576.836.096.000]
    [height=65.510 parent-min-ts=9.223.372.093.639.029.412 observed-target=3.983.078.313.091.565.135.719.559.240.834.096.108.275.259.772.194.089.812.166.606.631.342.797.192.259.804.048.916.480]
    [height=65.511 parent-min-ts=9.223.372.093.639.029.521 observed-target=3.980.407.143.364.754.758.247.044.980.712.213.769.778.626.462.790.393.120.324.885.389.279.406.198.867.646.887.231.488]
    [height=65.512 parent-min-ts=9.223.372.093.639.029.664 observed-target=3.979.982.184.544.580.380.012.781.348.420.096.125.017.798.436.294.350.464.804.611.555.314.775.813.555.258.247.872.512]
    [height=65.513 parent-min-ts=9.223.372.093.639.029.679 observed-target=3.971.422.299.738.210.761.294.042.469.393.154.994.835.405.331.159.776.975.039.095.756.884.363.766.548.572.797.927.424]
    [height=65.514 parent-min-ts=9.223.372.093.639.029.707 observed-target=3.963.681.978.370.748.872.027.097.738.358.155.036.691.751.991.410.428.606.634.108.066.814.310.319.787.208.295.317.504]
    [height=65.515 parent-min-ts=9.223.372.093.639.029.794 observed-target=3.959.675.223.780.533.305.818.326.348.175.331.528.946.802.027.304.883.568.871.526.203.719.223.829.698.972.552.790.016]
    [height=65.516 parent-min-ts=9.223.372.093.639.029.944 observed-target=3.959.675.223.780.533.305.818.326.348.175.331.528.946.802.027.304.883.568.871.526.203.719.223.829.698.972.552.790.016]
    [height=65.517 parent-min-ts=9.223.372.093.639.030.019 observed-target=3.954.939.968.355.733.091.207.960.159.777.449.201.611.861.160.634.693.978.788.474.910.970.485.250.503.784.857.075.712]
    [height=65.518 parent-min-ts=9.223.372.093.639.030.183 observed-target=3.955.820.240.197.522.874.693.220.540.953.978.608.616.433.501.233.639.479.509.042.138.468.648.191.508.018.467.176.448]
    [height=65.519 parent-min-ts=9.223.372.093.639.030.211 observed-target=3.948.110.273.031.502.012.443.008.926.511.272.767.955.696.449.091.151.300.784.074.007.967.496.915.126.110.295.949.312]
    [height=65.520 parent-min-ts=9.223.372.093.639.030.251 observed-target=3.941.159.160.901.506.825.611.125.226.875.919.864.367.866.587.120.167.864.059.594.866.688.899.898.230.610.409.291.776]
  ==
::
::  +compute-vector-target: run compute-target:asert with the cutover
::  constants for one vector's parent-min-ts / height.
++  compute-vector-target
  |=  v=[height=@ parent-min-ts=@ observed-target=@]
  ^-  @
  %-  compute-target:asert
  :*  anchor-target-atom
      anchor-min-timestamp
      anchor-height
      parent-min-ts.v
      height.v
      ideal-block-time
      half-life
      max-target-atom
  ==
::
::  +vectors-pass: %.y iff every vector's computed target equals its
::  observed target. Empty list trivially passes.
++  vectors-pass
  ^-  ?
  %+  levy  asert-vectors
  |=  v=[height=@ parent-min-ts=@ observed-target=@]
  =(observed-target.v (compute-vector-target v))
--
::
|%
::  +test-asert-mainnet-cross-check: every observed mainnet target for
::  post-activation blocks must equal what compute-target:asert produces
::  given the cutover constants. Asserts vacuously while asert-vectors
::  is empty; populate via scripts/aletheia_asert_vectors.rs once a
::  reachable public gRPC node is available, then this test pins
::  consensus continuity bit-for-bit.
++  test-asert-mainnet-cross-check
  (expect-eq !>(%.y) !>(vectors-pass))
::
::  +test-asert-anchor-pin-self-consistent: sanity-check that the
::  constants this file pins match the realnet bunt of
::  blockchain-constants:v1. Catches any drift between the test pins
::  and the production constants. Reads via *blockchain-constants:t —
::  if anyone changes a field default without updating this file, the
::  test fails with a clear diff.
++  test-asert-anchor-pin-self-consistent
  =/  bc  *blockchain-constants:txe
  ;:  weld
    %+  expect-eq  !>(anchor-min-timestamp)
    !>(anchor-min-timestamp.zk-asert.bc)
  ::
    %+  expect-eq  !>(anchor-target-atom)
    !>(anchor-target-atom.zk-asert.bc)
  ::
    %+  expect-eq  !>(anchor-height)
    !>(anchor-height.zk-asert.bc)
  ::
    %+  expect-eq  !>(ideal-block-time)
    !>(ideal-block-time.zk-asert.bc)
  ::
    %+  expect-eq  !>(half-life)
    !>(half-life.zk-asert.bc)
  ==
--
