# frozen_string_literal: true

module Avrocadabra
  module Logical
    EPOCH_JD = 2_440_588

    class << self
      def decimal_unscaled(value, precision, scale)
        sign, digits, _base, exponent = decimal_input(value).split
        return "0" if digits == "0"

        shift = exponent - digits.length + scale
        raise EncodeError, "decimal has excess scale (maximum #{scale})" if shift.negative?
        raise EncodeError, "decimal exceeds precision #{precision}" if digits.length + shift > precision

        "#{"-" if sign.negative?}#{digits}#{"0" * shift}"
      end

      def decimal_value(unscaled, scale)
        value = BigDecimal("#{unscaled}e#{-scale}")
        unless value.finite? && (!value.zero? || unscaled == "0")
          raise DecodeError, "decimal exponent exceeds Ruby BigDecimal range"
        end

        value
      end

      def big_decimal_parts(value)
        sign, digits, _base, exponent = decimal_input(value).split
        return ["0", 0] if digits == "0"

        [sign.negative? ? "-#{digits}" : digits, digits.length - exponent]
      end

      def date_days(value)
        return value.to_i if value.is_a?(Numeric)
        raise EncodeError, "date requires Date or Numeric" unless value.is_a?(Date)

        value.jd - EPOCH_JD
      end

      def date_value(days)
        Date.jd(EPOCH_JD + days, Date::GREGORIAN)
      end

      def timestamp_ticks(value, units)
        return value.to_i if value.is_a?(Numeric)
        raise EncodeError, "timestamp requires Time, Date or Numeric" unless value.respond_to?(:to_time)

        (value.to_time.to_r * units).floor
      end

      def timestamp_value(ticks, units)
        Time.at(Rational(ticks, units)).utc
      end

      private

      def decimal_input(value)
        unless value.is_a?(BigDecimal) || value.is_a?(Integer) || value.is_a?(Float)
          raise EncodeError, "decimal requires BigDecimal, Integer or Float"
        end

        value = value.to_d
        raise EncodeError, "decimal must be finite" unless value.finite?

        value
      end
    end
  end
end
