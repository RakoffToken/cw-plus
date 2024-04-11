use crate::{error::ContractError, state::get_commission};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{to_binary, Addr, Coin, Decimal, Deps, DepsMut, StdError, Uint128};
use cw20::Cw20Coin;
use std::convert::TryInto;

#[cw_serde]
pub enum Amount {
    Native(Coin),
    // FIXME? USe Cw20CoinVerified, and validate cw20 addresses
    Cw20(Cw20Coin),
}

impl Amount {
    // TODO: write test for this
    pub fn from_parts(denom: String, amount: Uint128) -> Self {
        if denom.starts_with("cw20:") {
            let address = denom.get(5..).unwrap().into();
            Amount::Cw20(Cw20Coin { address, amount })
        } else {
            Amount::Native(Coin { denom, amount })
        }
    }

    pub fn cw20(amount: u128, addr: &str) -> Self {
        Amount::Cw20(Cw20Coin {
            address: addr.into(),
            amount: Uint128::new(amount),
        })
    }

    pub fn native(amount: u128, denom: &str) -> Self {
        Amount::Native(Coin {
            denom: denom.to_string(),
            amount: Uint128::new(amount),
        })
    }
}

impl Amount {
    pub fn denom(&self) -> String {
        match self {
            Amount::Native(c) => c.denom.clone(),
            Amount::Cw20(c) => format!("cw20:{}", c.address.as_str()),
        }
    }

    pub fn amount(&self) -> Uint128 {
        match self {
            Amount::Native(c) => c.amount,
            Amount::Cw20(c) => c.amount,
        }
    }

    /// convert the amount into u64
    pub fn u64_amount(&self) -> Result<u64, ContractError> {
        Ok(self.amount().u128().try_into()?)
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Amount::Native(c) => c.amount.is_zero(),
            Amount::Cw20(c) => c.amount.is_zero(),
        }
    }

    pub fn into_cosmos_msg(&self, addr: Addr) -> Result<Option<cosmwasm_std::CosmosMsg>, ContractError> {
        if self.amount().is_zero() {
            return Ok(None);
        }
        match self {
            Amount::Native(c) => Ok(Some(cosmwasm_std::CosmosMsg::Bank(cosmwasm_std::BankMsg::Send {
                to_address: addr.into(),
                amount: vec![c.clone()],
            })),
            ),
            Amount::Cw20(c) => Ok(Some(cosmwasm_std::CosmosMsg::Wasm(cosmwasm_std::WasmMsg::Execute {
                contract_addr: c.address.clone(),
                msg: to_binary(&cw20::Cw20ExecuteMsg::Transfer {
                    recipient: addr.into(),
                    amount: c.amount,
                })?,
                funds: vec![],
            })),
            ),
        }
    }
}

pub fn calculate_lock_in(
    deps: Deps,
    amount: Amount,
) -> Result<(Amount, Amount), ContractError>  {

    if amount.is_empty() {
        return Err(ContractError::NoFunds {});
    }
    let comm = get_commission(deps.storage)?;
    let amnt_decimal = Decimal::from_ratio(amount.amount(), 1u32);
    let lock_in = comm.checked_mul(amnt_decimal).map_err(|_| StdError::generic_err("error multiplying1"))?;
    let lock_in = lock_in.ceil();
    let lock_in_uint: Uint128 = lock_in * Uint128::one();
    let transfer_amnt_uint = amount.amount().checked_sub(lock_in_uint).map_err(|_| StdError::generic_err("error subtracting"))?;
    
    let lock_in_amount = match amount.clone() {
        Amount::Native(c) => {
            let coin = Coin {
                denom: c.denom,
                amount: lock_in_uint,
            };
            Amount::Native(
                coin
            )
        }
        Amount::Cw20(coin) => {
            let coin = Cw20Coin {
                address: coin.address.clone(),
                amount: lock_in_uint,
            };
            Amount::Cw20(
                coin
            )
        }
    };

    let transfer_amount = match amount.clone() {
        Amount::Native(c) => {
            let coin = Coin {
                denom: c.denom,
                amount: transfer_amnt_uint,
            };
            Amount::Native(
                coin
            )
        }
        Amount::Cw20(coin) => {
            let coin = Cw20Coin {
                address: coin.address.clone(),
                amount: transfer_amnt_uint,
            };
            Amount::Cw20(
                coin
            )
        }
    };

    return Ok((lock_in_amount, transfer_amount));
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::QuerierWrapper;
    use cosmwasm_std::{testing::mock_dependencies, WasmMsg};

    use crate::{msg::ExecuteMsg, state::set_commission};
    use super::calculate_lock_in;

    use super::*;
    #[test]
    fn test_into_cosmos_msg_native() {
        let amount = Amount::native(100, "uusd");
        let addr = "recipient_address";

        let msg = amount.into_cosmos_msg(Addr::unchecked(addr)).unwrap();

        match msg {
            Some(cosmwasm_std::CosmosMsg::Bank(bank_msg)) => {
                match bank_msg {
                    cosmwasm_std::BankMsg::Send { to_address, amount } => {
                        assert_eq!(to_address, addr);
                        assert_eq!(amount.len(), 1);
                        assert_eq!(amount[0].denom, "uusd");
                        assert_eq!(amount[0].amount, Uint128::new(100));
                    }
                    _ => panic!("Unexpected BankMsg variant"),
                }
            }
            _ => panic!("Unexpected CosmosMsg variant"),
        }
    }

    #[test]
    fn test_into_cosmos_msg_cw20() {
        let amount = Amount::cw20(100, "contract_address");
        let addr = "recipient_address";

        let msg = amount.into_cosmos_msg(Addr::unchecked(addr)).unwrap();

        match msg {
            Some(cosmwasm_std::CosmosMsg::Wasm(wasm_msg)) => {
                match wasm_msg {
                    WasmMsg::Execute{contract_addr, msg, funds} => {
                        assert_eq!(contract_addr, "contract_address");
                        assert_eq!(
                            msg,
                            to_binary(&cw20::Cw20ExecuteMsg::Transfer {
                                recipient: addr.into(),
                                amount: Uint128::new(100),
                            })
                            .unwrap()
                        );
                        assert_eq!(funds.len(), 0);
                    },
                    _ => panic!("Unexpected WasmMsg variant"),
                }
            }
            _ => panic!("Unexpected CosmosMsg variant"),
        }
    }

    #[test]
    fn test_calculate_lock_in_native() {
        let mut owned = mock_dependencies();
        set_commission(&mut owned.storage, Decimal::percent(10)).unwrap();
        let deps = DepsMut {
            storage: &mut owned.storage,
            api: &owned.api,
            querier: QuerierWrapper::new(&owned.querier),
        };
        let reference = deps.as_ref();

        let amount = Amount::Native(Coin {
            denom: "uusd".to_string(),
            amount: Uint128::new(100),
        });
        let lock_in_amount = calculate_lock_in(reference, amount).unwrap();

        match lock_in_amount {
            (Amount::Native(coin_lock), Amount::Native(coin_transfer)) => {
                assert_eq!(coin_lock.denom, "uusd");
                assert_eq!(coin_lock.amount, Uint128::new(10));
                assert_eq!(coin_transfer.denom, "uusd");
                assert_eq!(coin_transfer.amount, Uint128::new(90));
            }
            _ => panic!("Unexpected Amount variant"),
        }
    }

    #[test]
    fn test_calculate_lock_in_cw20() {
        let mut owned = mock_dependencies();
        set_commission(&mut owned.storage, Decimal::percent(10)).unwrap();
        let deps = DepsMut {
            storage: &mut owned.storage,
            api: &owned.api,
            querier: QuerierWrapper::new(&owned.querier),
        };
        let reference = deps.as_ref();

        let amount = Amount::Cw20(Cw20Coin {
            address: "contract_address".to_string(),
            amount: Uint128::new(100),
        });
        let lock_in_amount = calculate_lock_in(reference, amount).unwrap();

        match lock_in_amount {
            (Amount::Cw20(coin_lock_in), Amount::Cw20(coin_transfer)) => {
                assert_eq!(coin_lock_in.address, "contract_address");
                assert_eq!(coin_lock_in.amount, Uint128::new(10));
                assert_eq!(coin_transfer.address, "contract_address");
                assert_eq!(coin_transfer.amount, Uint128::new(90));
            }
            _ => panic!("Unexpected Amount variant"),
        }
    }

}
