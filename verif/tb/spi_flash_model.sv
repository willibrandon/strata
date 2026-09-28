// Behavioral SPI NOR flash (testbench only). Supports READ (0x03) with a
// 24-bit address, JEDEC ID (0x9F) and release from power-down (0xAB).
// Unwritten locations read as 0xFF, like erased flash.
module spi_flash_model #(
  parameter int SIZE = 4 * 1024 * 1024
) (
  input  logic cs_n,
  input  logic sck,
  input  logic mosi,
  output logic miso
);

  logic [7:0] mem [SIZE];

  initial begin
    string path;
    int    offset;
    for (int i = 0; i < SIZE; i++) mem[i] = 8'hFF;
    if ($value$plusargs("flash=%s", path)) begin
      if (!$value$plusargs("flash_offset=%h", offset)) offset = 32'h0020_0000;
      $readmemh(path, mem, offset);
    end
  end

  typedef enum { CMD, ADDR, DATA, ID } state_e;
  state_e      state;
  logic [7:0]  sh_in, sh_out;
  int          nbits, nbytes;
  logic [23:0] addr;

  always @(posedge cs_n) state = CMD;

  always @(negedge cs_n) begin
    state = CMD; nbits = 0; nbytes = 0; miso = 1'b0;
  end

  always @(posedge sck) if (!cs_n) begin
    sh_in = {sh_in[6:0], mosi};
    nbits++;
    if (nbits == 8) begin
      nbits = 0;
      case (state)
        CMD: begin
          if (sh_in == 8'h03) begin state = ADDR; nbytes = 0; end
          else if (sh_in == 8'h9F) begin state = ID; nbytes = 0; sh_out = 8'hEF; end
          else state = CMD;
        end
        ADDR: begin
          addr = {addr[15:0], sh_in};
          nbytes++;
          if (nbytes == 3) begin state = DATA; sh_out = mem[addr]; addr++; end
        end
        DATA: begin sh_out = mem[addr]; addr++; end
        ID:   begin nbytes++; sh_out = (nbytes == 1) ? 8'h40 : 8'h18; end
      endcase
    end
  end

  // Mode 0: the next bit is driven after the falling edge.
  always @(negedge sck) if (!cs_n) begin
    if (state == DATA || state == ID) begin
      miso = sh_out[7 - nbits];
    end
  end

endmodule
