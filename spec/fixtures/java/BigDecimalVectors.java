import java.io.ByteArrayOutputStream;
import java.math.BigDecimal;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.Map;
import org.apache.avro.Conversions.BigDecimalConversion;
import org.apache.avro.io.BinaryDecoder;
import org.apache.avro.io.BinaryEncoder;
import org.apache.avro.io.DecoderFactory;
import org.apache.avro.io.EncoderFactory;

class BigDecimalVectors {
  private static final BigDecimalConversion CONVERSION = new BigDecimalConversion();

  private static byte[] encode(BigDecimal value) throws Exception {
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    BinaryEncoder encoder = EncoderFactory.get().binaryEncoder(out, null);
    encoder.writeBytes(CONVERSION.toBytes(value, null, null));
    encoder.flush();
    return out.toByteArray();
  }

  private static BigDecimal decode(byte[] bytes) throws Exception {
    BinaryDecoder decoder = DecoderFactory.get().binaryDecoder(bytes, null);
    ByteBuffer inner = decoder.readBytes(null);
    if (!decoder.isEnd()) throw new AssertionError("Trailing datum bytes");
    return CONVERSION.fromBytes(inner, null, null);
  }

  private static void verify(Path path) throws Exception {
    int count = 0;
    for (String line : Files.readAllLines(path)) {
      String[] columns = line.split("\t");
      BigDecimal expected = new BigDecimal(new BigInteger(columns[0]), Integer.parseInt(columns[1]));
      BigDecimal actual = decode(HexFormat.of().parseHex(columns[2]));
      if (actual.compareTo(expected) != 0) throw new AssertionError("Decimal differs: " + line);
      count++;
    }
    System.out.println("Java decoded " + count + " native big-decimal datums exactly");
  }

  public static void main(String[] args) throws Exception {
    if (args.length == 2 && args[0].equals("verify")) {
      verify(Path.of(args[1]));
      return;
    }
    Map<String, BigDecimal> values = new LinkedHashMap<>();
    values.put("zero", new BigDecimal("0"));
    values.put("scaled_zero", new BigDecimal("0.0000"));
    values.put("one", new BigDecimal("1"));
    values.put("negative_one", new BigDecimal("-1"));
    values.put("positive_127", new BigDecimal("127"));
    values.put("positive_128", new BigDecimal("128"));
    values.put("negative_128", new BigDecimal("-128"));
    values.put("negative_129", new BigDecimal("-129"));
    values.put("negative_scale", new BigDecimal("1.23E+8"));
    values.put("fractional", new BigDecimal("0.0000123456789"));
    values.put("trailing_zeros", new BigDecimal("1.2300"));
    values.put("integer_zeros", new BigDecimal("1200"));
    values.put("large_precision", new BigDecimal("123456789012345678901234567890.123456789012345678901234567890"));
    values.put("negative_precision", new BigDecimal("-987654321098765432109876543210987654321.9876543210987654321"));
    values.put("huge_magnitude", new BigDecimal(BigInteger.valueOf(123), -1_000_000));
    values.put("tiny_magnitude", new BigDecimal(BigInteger.valueOf(-987654321), 1_000_000));
    values.put("maximum_java_scale", new BigDecimal(BigInteger.ONE, Integer.MAX_VALUE));
    values.put("minimum_java_scale", new BigDecimal(BigInteger.ONE, Integer.MIN_VALUE));

    System.out.println("{\n  \"producer\": \"Apache Avro Java 1.12.2 BigDecimalConversion\",\n  \"vectors\": [");
    boolean first = true;
    for (Map.Entry<String, BigDecimal> entry : values.entrySet()) {
      BigDecimal value = entry.getValue();
      BigDecimal canonical = value.stripTrailingZeros();
      byte[] wire = encode(value);
      byte[] canonicalWire = encode(canonical);
      if (!decode(wire).equals(value) || !decode(canonicalWire).equals(canonical)) {
        throw new AssertionError("Java conversion changed coefficient or scale");
      }
      if (!first) System.out.println(",");
      first = false;
      System.out.printf("    {\"name\":\"%s\",\"value\":\"%s\",\"coefficient\":\"%s\",\"scale\":%d,"
          + "\"wire_hex\":\"%s\",\"canonical_coefficient\":\"%s\",\"canonical_scale\":%d,"
          + "\"canonical_wire_hex\":\"%s\"}", entry.getKey(), value, value.unscaledValue(), value.scale(),
          HexFormat.of().formatHex(wire), canonical.unscaledValue(), canonical.scale(),
          HexFormat.of().formatHex(canonicalWire));
    }
    System.out.println("\n  ]\n}");
  }
}
