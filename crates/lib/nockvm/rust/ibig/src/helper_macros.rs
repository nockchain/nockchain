/// Implement impl Op<B> for &A by forwarding to impl Op<B> for A.
/// Includes &B.
macro_rules! forward_binop_first_arg_by_value {
    (impl $tr:ident<$t2:ty> for $t1:ty, $f:ident) => {
        impl $tr<$t2> for &$t1 {
            type Output = <$t1 as $tr<$t2>>::Output;

            #[inline]
            fn $f(self, rhs: $t2) -> Self::Output {
                (*self).$f(rhs)
            }
        }

        impl<'a> $tr<&'a $t2> for &$t1 {
            type Output = <$t1 as $tr<&'a $t2>>::Output;

            #[inline]
            fn $f(self, rhs: &$t2) -> Self::Output {
                (*self).$f(rhs)
            }
        }
    };
}

/// Implement impl Op<&B> for A by forwarding to impl Op<B> for A.
/// Includes &A.
macro_rules! forward_binop_second_arg_by_value {
    (impl $tr:ident<$t2:ty> for $t1:ty, $f:ident) => {
        impl $tr<&$t2> for $t1 {
            type Output = <$t1 as $tr<$t2>>::Output;

            #[inline]
            fn $f(self, rhs: &$t2) -> Self::Output {
                self.$f(*rhs)
            }
        }

        impl<'a> $tr<&$t2> for &'a $t1 {
            type Output = <&'a $t1 as $tr<$t2>>::Output;

            #[inline]
            fn $f(self, rhs: &$t2) -> Self::Output {
                self.$f(*rhs)
            }
        }
    };
}

/// Implement impl Op<&B> for A by forwarding to impl Op<B> for A.
/// Here Op has OutputDiv and OutputRem, rather than just Output.
/// Includes &A.
macro_rules! forward_div_rem_second_arg_by_value {
    (impl $tr:ident<$t2:ty> for $t1:ty, $f:ident) => {
        impl $tr<&$t2> for $t1 {
            type OutputDiv = <$t1 as $tr<$t2>>::OutputDiv;
            type OutputRem = <$t1 as $tr<$t2>>::OutputRem;

            #[inline]
            fn $f(self, rhs: &$t2) -> (Self::OutputDiv, Self::OutputRem) {
                self.$f(*rhs)
            }
        }

        impl<'a> $tr<&$t2> for &'a $t1 {
            type OutputDiv = <&'a $t1 as $tr<$t2>>::OutputDiv;
            type OutputRem = <&'a $t1 as $tr<$t2>>::OutputRem;

            #[inline]
            fn $f(self, rhs: &$t2) -> (Self::OutputDiv, Self::OutputRem) {
                self.$f(*rhs)
            }
        }
    };
}

/// Implement impl Op<B> for A by forwarding to impl Op<A> for B.
/// Includes &A and &B.
macro_rules! forward_binop_swap_args {
    (impl $tr:ident<$t2:ty> for $t1:ty, $f:ident) => {
        impl $tr<$t2> for $t1 {
            type Output = <$t2 as $tr<$t1>>::Output;

            #[inline]
            fn $f(self, rhs: $t2) -> Self::Output {
                rhs.$f(self)
            }
        }

        impl<'a> $tr<&'a $t2> for $t1 {
            type Output = <&'a $t2 as $tr<$t1>>::Output;

            #[inline]
            fn $f(self, rhs: &$t2) -> Self::Output {
                rhs.$f(self)
            }
        }

        impl<'a> $tr<$t2> for &'a $t1 {
            type Output = <$t2 as $tr<&'a $t1>>::Output;

            #[inline]
            fn $f(self, rhs: $t2) -> Self::Output {
                rhs.$f(self)
            }
        }

        impl<'a, 'b> $tr<&'a $t2> for &'b $t1 {
            type Output = <&'a $t2 as $tr<&'b $t1>>::Output;

            #[inline]
            fn $f(self, rhs: &$t2) -> Self::Output {
                rhs.$f(self)
            }
        }
    };
}

/// Implement impl OpAssign<&B> for A by forwarding to impl OpAssign<B> for A.
macro_rules! forward_binop_assign_arg_by_value {
    (impl $tr:ident<$t2:ty> for $t1:ty, $f:ident) => {
        impl $tr<&$t2> for $t1 {
            #[inline]
            fn $f(&mut self, rhs: &$t2) {
                self.$f(*rhs)
            }
        }
    };
}

#[rustfmt::skip]
pub(crate) use {
    forward_binop_assign_arg_by_value, forward_binop_first_arg_by_value,
    forward_binop_second_arg_by_value, forward_binop_swap_args,
    forward_div_rem_second_arg_by_value,
};
