// Reset generation: synchronizes the external reset request and merges the
// software reset request from SYS. All logic uses synchronous, active-high
// reset; one cycle of rst is sufficient.
module st_reset (
  input  logic clk,
  input  logic rst_in,      // asynchronous request, active high
  input  logic soft_req,    // registered one-cycle request from SYS
  output logic rst
);

  logic [1:0] sync_q;

  always_ff @(posedge clk) sync_q <= {sync_q[0], rst_in};

  assign rst = sync_q[1] || soft_req;

endmodule
