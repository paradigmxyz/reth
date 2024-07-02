pragma solidity ^0.7.0;

contract Test {
    function a() public {
    	assembly {
		    function power(base, exponent) -> result {
		        switch exponent
		        case 0 { result := 1 }
		        case 1 { result := base }
		        default {
		            result := power(mul(base, base), div(exponent, 2))
		            switch mod(exponent, 2)
		                case 1 { result := mul(base, result) }
		        }
	    	}
    	}
    }
}
