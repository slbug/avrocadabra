# frozen_string_literal: true

RSpec.describe Avrocadabra::Logical do
  it "explains invalid date input without leaking method lookup errors" do
    [nil, "1970-01-01", Time.at(0)].each do |value|
      expect { described_class.date_days(value) }.to raise_error(Avrocadabra::EncodeError, /date requires/)
    end
  end

  it "explains invalid timestamp input without leaking method lookup errors" do
    [nil, "1970-01-01", Object.new].each do |value|
      expect { described_class.timestamp_ticks(value, 1000) }
        .to raise_error(Avrocadabra::EncodeError, /timestamp requires/)
    end
  end

  describe ".decimal_unscaled" do
    ["1.25", nil, :value].each do |value|
      it "rejects #{value.inspect} with an explicit Ruby type requirement" do
        expect { described_class.decimal_unscaled(value, 4, 2) }
          .to raise_error(Avrocadabra::EncodeError, /BigDecimal, Integer or Float/)
      end
    end

    %w[NaN Infinity -Infinity].each do |text|
      it "identifies non-finite #{text} before converting decimal digits" do
        expect { described_class.decimal_unscaled(BigDecimal(text), 4, 2) }
          .to raise_error(Avrocadabra::EncodeError, /finite/)
      end
    end
  end
end
